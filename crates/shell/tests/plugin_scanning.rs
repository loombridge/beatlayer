//! The plug-in scanner's commands through the shell (WP-2.9; the plug-in
//! scanning spec, R1, R2, R4, R6, R7, R8): `plugin.scan` finds the test
//! plug-in and caches it, `plugin.rescan` scans again, a set's list of
//! plug-ins comes from `plugin.list` (IMP-29), and the hidden and format
//! preferences come from `plugin.settings` (IMP-07), all through registered
//! commands as an agent runs them. The browser files plug-ins by vendor,
//! lists a format only while its switch is on, and selects and loads them
//! (WP-2.9b; R11, R12).

use bl_model::PluginFormat;
use bl_shell::Shell;
use bl_shell::browser_commands as browser_ids;
use bl_shell::plugin_commands::{self as ids, PluginScan, PluginSettings, SetPluginList};
use serde_json::json;

/// The plug-in scanner's child process is built beside these tests; a run
/// that didn't build it can't scan.
fn ready() -> bool {
    bl_plugin_host::scan::scan_program().is_some()
}

#[test]
fn scanning_rescanning_and_the_set_list_work_through_commands() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();

    // Spec: plugin-scanning R1, R3. A scan finds the test plug-in.
    let scan: PluginScan = serde_json::from_value(
        shell
            .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(scan.scanned, 1);
    assert_eq!(scan.plugins.len(), 1);
    assert_eq!(scan.plugins[0].plugin_id, bl_test_clap::PLUGIN_ID);
    // Spec: plugin-scanning R1; IMP-24. The scan says whether it sends notes
    // on from a note output.
    assert!(scan.plugins[0].note_output);

    // Spec: plugin-scanning R2. A second scan is answered from the cache.
    let cached: PluginScan = serde_json::from_value(
        shell
            .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cached.scanned, 0);
    assert_eq!(cached.cached, 1);

    // Spec: plugin-scanning R6. Rescan scans again.
    let rescanned: PluginScan = serde_json::from_value(
        shell
            .run(ids::RESCAN, json!({ "paths": [bundle.clone()] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(rescanned.scanned, 1);

    // Spec: plugin-scanning R8. The set's list finds the plug-in a device
    // uses, and says this machine has it.
    let track = shell
        .editor()
        .document()
        .tracks()
        .next()
        .map(bl_model::Track::id)
        .expect("the default set has a track");
    let _loaded: serde_json::Value = shell
        .run(ids::LOAD, json!({ "path": bundle, "track": track }))
        .unwrap();
    let list: SetPluginList =
        serde_json::from_value(shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins.len(), 1);
    assert_eq!(list.plugins[0].plugin_id, bl_test_clap::PLUGIN_ID);
    assert!(!list.plugins[0].missing, "the scanner knows it");
    assert!(list.plugins[0].blocked.is_none());
}

// Spec: plugin-scanning R1, R7, R10. The scanner learns VST3 (WP-2.7):
// `plugin.scan` finds VST3 plug-ins beside CLAP's, and the format
// preference (IMP-07) keeps one when a plug-in is offered as both.
#[test]
fn a_scan_finds_vst3_plug_ins_and_the_format_preference_chooses() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let clap = bl_test_clap::bundle().to_string_lossy().into_owned();
    let vst3 = bl_test_vst3::bundle().to_string_lossy().into_owned();

    let scan: PluginScan = serde_json::from_value(
        shell
            .run(ids::SCAN, json!({ "paths": [clap, vst3] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(scan.plugins.len(), 2, "{:?}", scan.plugins);
    assert!(
        scan.plugins
            .iter()
            .any(|p| p.format == PluginFormat::Vst3 && p.plugin_id == bl_test_vst3::plugin_id()),
        "{:?}",
        scan.plugins
    );

    // Spec: plugin-scanning R7. Both test plug-ins share a name, so the
    // format preference leaves one in the browser.
    let settings: PluginSettings = serde_json::from_value(
        shell
            .run(ids::SETTINGS, json!({ "format_preference": "vst3" }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings.format_preference, Some(PluginFormat::Vst3));
    // Spec: plugin-scanning R12. VST3 is listed once its switch is on.
    shell
        .run(ids::SETTINGS, json!({ "vst3_system_folders": true }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    let state = browser_state(&mut shell);
    assert_eq!(state.total_rows, 1, "one vendor folder: {state:?}");
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": state.rows[0].path, "open": true }),
        )
        .unwrap();
    let state = browser_state(&mut shell);
    assert_eq!(state.total_rows, 2, "the folder and one plug-in: {state:?}");
    assert_eq!(state.rows[1].format, Some(PluginFormat::Vst3));
}

fn browser_state(shell: &mut Shell) -> bl_shell::browser_commands::BrowserState {
    serde_json::from_value(shell.run(browser_ids::STATE, json!({})).unwrap()).unwrap()
}

// Spec: plugin-scanning R12. CLAP is always listed; VST3 and Audio Units
// only while Settings ▸ Plug-Ins switches them on, and all are off in a new
// install, as L-SET-017 records Live's defaults.
#[test]
fn the_plug_in_sources_switches_choose_the_formats_listed() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let settings: PluginSettings =
        serde_json::from_value(shell.run(ids::SETTINGS, json!({})).unwrap()).unwrap();
    assert!(!settings.use_audio_units_v2);
    assert!(!settings.use_audio_units_v3);
    assert!(!settings.vst3_system_folders);
    let clap = bl_test_clap::bundle().to_string_lossy().into_owned();
    let vst3 = bl_test_vst3::bundle().to_string_lossy().into_owned();
    shell
        .run(ids::SCAN, json!({ "paths": [clap, vst3] }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    let folder = browser_state(&mut shell).rows[0].path.clone();
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": folder, "open": true }),
        )
        .unwrap();
    let formats = |shell: &mut Shell| -> Vec<PluginFormat> {
        browser_state(shell)
            .rows
            .iter()
            .filter_map(|row| row.format)
            .collect()
    };
    assert_eq!(formats(&mut shell), [PluginFormat::Clap]);
    let settings: PluginSettings = serde_json::from_value(
        shell
            .run(ids::SETTINGS, json!({ "vst3_system_folders": true }))
            .unwrap(),
    )
    .unwrap();
    assert!(settings.vst3_system_folders);
    let mut listed = formats(&mut shell);
    listed.sort_by_key(|f| format!("{f:?}"));
    assert_eq!(listed, [PluginFormat::Clap, PluginFormat::Vst3]);
    shell
        .run(ids::SETTINGS, json!({ "vst3_system_folders": false }))
        .unwrap();
    assert_eq!(formats(&mut shell), [PluginFormat::Clap]);
}

// Spec: plugin-scanning R11. The Plug-Ins label files plug-ins in a folder
// per vendor, as Live 12 does; a plug-in row is selected by its key without
// an error, and Enter (`browser.load_selected`) loads it onto the selected
// track, or in place of the device being hot-swapped.
#[test]
fn a_scanned_plug_in_is_selected_in_the_browser_and_loads_onto_a_track() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    shell
        .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    let state = browser_state(&mut shell);
    assert_eq!(state.rows.len(), 1, "{state:?}");
    let folder = &state.rows[0];
    assert_eq!(folder.name, "Beatlayer tests");
    assert_eq!(folder.kind, browser_ids::ItemKind::Folder);
    assert!(!folder.open, "vendor folders start closed");

    // Selecting the folder and opening it work on its key.
    shell
        .run(browser_ids::SELECT, json!({ "paths": [folder.path] }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": folder.path }))
        .unwrap();
    let state = browser_state(&mut shell);
    assert_eq!(state.rows.len(), 2, "{state:?}");
    let row = state.rows[1].clone();
    assert_eq!(row.kind, browser_ids::ItemKind::Plugin);
    assert_eq!(row.depth, 1);
    assert_eq!(row.format, Some(PluginFormat::Clap));
    assert_eq!(row.path, format!("{bundle}#{}", bl_test_clap::PLUGIN_ID));

    // Selecting the plug-in is no error, and nothing else changes.
    let selected: bl_shell::browser_commands::BrowserState = serde_json::from_value(
        shell
            .run(browser_ids::SELECT, json!({ "paths": [row.path] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(selected.selected, vec![row.path.clone()]);

    // With no track selected, it loads onto a new track (audio, for the
    // test plug-in's audio effect), as Live, which always has a track
    // selected, never refuses.
    shell
        .run(
            bl_shell::arrangement_commands::SELECT_TRACKS,
            json!({ "tracks": [] }),
        )
        .unwrap();
    let tracks_before = shell.editor().document().tracks().count();
    let first: bl_shell::plugin_commands::PluginLoaded =
        serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert_eq!(
        shell.editor().document().tracks().count(),
        tracks_before + 1
    );
    let new_track = shell.editor().document().track(first.track).unwrap();
    assert_eq!(new_track.kind(), bl_model::TrackKind::Audio);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().tracks().count(),
        tracks_before,
        "the new track and the plug-in are one undo step"
    );

    // Enter loads it onto the selected track.
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .run(
            bl_shell::arrangement_commands::SELECT_TRACKS,
            json!({ "tracks": [track] }),
        )
        .unwrap();
    let loaded = shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let loaded: bl_shell::plugin_commands::PluginLoaded = serde_json::from_value(loaded).unwrap();
    assert_eq!(loaded.plugin, bl_test_clap::PLUGIN_ID);
    assert_eq!(loaded.track, track);
    let plugin_devices = |shell: &Shell| -> Vec<bl_model::DeviceId> {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .devices()
            .filter(|d| {
                matches!(&d.kind, bl_model::DeviceKind::Plugin { plugin_id, .. }
                    if plugin_id == bl_test_clap::PLUGIN_ID)
            })
            .map(|d| d.id)
            .collect()
    };
    assert_eq!(plugin_devices(&shell), [loaded.device]);

    // While the device is hot-swapped, the plug-in loads in its place.
    shell
        .run(
            bl_shell::device_commands::HOT_SWAP,
            json!({ "device": loaded.device, "on": true }),
        )
        .unwrap();
    let swapped: bl_shell::plugin_commands::PluginLoaded =
        serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert_eq!(plugin_devices(&shell), [swapped.device]);
    assert_ne!(swapped.device, loaded.device);

    // A preview of a plug-in is refused with a reason, not a missing file.
    let error = shell
        .run(browser_ids::PREVIEW, json!({ "path": row.path }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't an audio file"), "{error}");
}

#[test]
fn a_plugin_that_fails_a_scan_is_blocklisted_and_reported() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    // The blocklist is exercised in `bl-plugin-host`'s scan tests with the
    // crash and hang modes; here the command path reports a bad file.
    let dir = std::env::temp_dir().join(format!("bl-shell-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Broken.clap");
    std::fs::write(&path, b"not a clap entry").unwrap();
    let mut shell = Shell::headless();
    let scan: PluginScan = serde_json::from_value(
        shell
            .run(ids::SCAN, json!({ "paths": [path.to_string_lossy()] }))
            .unwrap(),
    )
    .unwrap();
    assert!(scan.plugins.is_empty());
    assert_eq!(scan.blocked.len(), 1);
    assert!(!scan.blocked[0].reason.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R1, R7; au-hosting's second part. The commands list
// Apple's built-in Audio Units, and they join the browser's Plug-Ins label.
#[cfg(target_os = "macos")]
#[test]
fn scanning_finds_audio_units_and_they_join_the_browser() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let scan: PluginScan = serde_json::from_value(
        shell
            .run(
                ids::SCAN,
                json!({ "paths": ["/System/Library/Components"] }),
            )
            .unwrap(),
    )
    .unwrap();
    let delay = scan
        .plugins
        .iter()
        .find(|p| p.plugin_id == "aufx/dely/appl")
        .expect("AUDelay among Apple's built-in units");
    assert_eq!(delay.format, bl_model::PluginFormat::AudioUnit);
    assert!(delay.path.ends_with(".component"), "{:?}", delay.path);

    // Spec: plugin-scanning R7, R11, R12. With Use Audio Units v2 on, the
    // Plug-Ins label lists it in Apple's folder.
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    assert_eq!(browser_state(&mut shell).total_rows, 0, "AU is off");
    shell
        .run(ids::SETTINGS, json!({ "use_audio_units_v2": true }))
        .unwrap();
    let state = browser_state(&mut shell);
    assert!(
        state.rows.iter().any(|row| row.name == "Apple"),
        "{state:?}"
    );
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": "plug-ins:/Apple", "open": true }),
        )
        .unwrap();
    let state = browser_state(&mut shell);
    assert!(
        state
            .rows
            .iter()
            .any(|row| row.name == "AUDelay"
                && row.format == Some(bl_model::PluginFormat::AudioUnit)),
        "{state:?}"
    );

    // Spec: plugin-scanning R11. An instrument loaded from the browser
    // replaces the selected track's instrument, as Live's browser does.
    let dls = state
        .rows
        .iter()
        .find(|row| row.name == "DLSMusicDevice")
        .expect("Apple's DLS instrument")
        .path
        .clone();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .run(
            bl_shell::arrangement_commands::SELECT_TRACKS,
            json!({ "tracks": [track] }),
        )
        .unwrap();
    shell
        .run(browser_ids::SELECT, json!({ "paths": [dls] }))
        .unwrap();
    let instruments = |shell: &Shell| -> Vec<bl_model::DeviceId> {
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .devices()
            .filter(|d| d.role == bl_model::DeviceRole::Instrument)
            .map(|d| d.id)
            .collect()
    };
    let first: bl_shell::plugin_commands::PluginLoaded =
        serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert_eq!(instruments(&shell), [first.device]);
    let second: bl_shell::plugin_commands::PluginLoaded =
        serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert_eq!(instruments(&shell), [second.device], "replaced, not added");
}

#[test]
fn hidden_plug_ins_leave_the_browser_and_come_back_with_show_hidden() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    shell
        .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
        .unwrap();

    // Spec: plugin-scanning R7. The Plug-Ins label lists it.
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    let state = |shell: &mut Shell| -> bl_shell::browser_commands::BrowserState {
        serde_json::from_value(shell.run(browser_ids::STATE, json!({})).unwrap()).unwrap()
    };
    assert_eq!(state(&mut shell).total_rows, 1);

    // Hide it, and it leaves the browser.
    let key = format!("clap/{}", bl_test_clap::PLUGIN_ID);
    let settings: PluginSettings = serde_json::from_value(
        shell
            .run(ids::SETTINGS, json!({ "hidden": [key] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings.hidden, vec![key.clone()]);
    assert_eq!(state(&mut shell).total_rows, 0);

    // Show hidden, and it comes back.
    shell
        .run(ids::SETTINGS, json!({ "show_hidden": true }))
        .unwrap();
    assert_eq!(state(&mut shell).total_rows, 1);
}

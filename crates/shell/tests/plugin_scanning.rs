//! The plug-in scanner's commands through the shell (WP-2.9; the plug-in
//! scanning spec, R1, R2, R4, R6, R7, R8): `plugin.scan` finds the test
//! plug-in and caches it, `plugin.rescan` scans again, a set's list of
//! plug-ins comes from `plugin.list` (IMP-29), and the hidden and format
//! preferences come from `plugin.settings` (IMP-07), all through registered
//! commands as an agent runs them.

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

    // Spec: plugin-scanning R7. The Plug-Ins label lists it.
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    let state: bl_shell::browser_commands::BrowserState =
        serde_json::from_value(shell.run(browser_ids::STATE, json!({})).unwrap()).unwrap();
    assert!(state.total_rows >= 1, "{state:?}");
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

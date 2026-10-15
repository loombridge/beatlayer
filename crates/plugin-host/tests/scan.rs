//! The plug-in scanner (WP-2.9; the plug-in scanning spec). Each plug-in is
//! described in the `beatlayer-scan` child process, so a plug-in that crashes
//! or hangs during a scan is blocklisted instead of taking Beatlayer down,
//! and unchanged files are answered from a cache.

use std::path::PathBuf;
use std::time::Duration;

use bl_model::PluginFormat;
use bl_plugin_host::scan::{BlockedPlugin, ScanReport, Scanner};
use bl_test_clap::{PLUGIN_ID, PLUGIN_NAME, TEST_SCAN_VAR, bundle};

/// The scanner binary Cargo built beside these tests.
const SCANNER: &str = env!("CARGO_BIN_EXE_beatlayer-scan");

fn scanner() -> Scanner {
    Scanner::new().with_program(SCANNER)
}

fn clap() -> PluginFormat {
    PluginFormat::Clap
}

// Spec: plugin-scanning R1, R3.
#[test]
fn a_scan_describes_the_test_plugin_and_caches_it() {
    let scanner = scanner();
    let paths = [bundle().to_path_buf()];
    let first = scanner.scan(clap(), &paths);
    assert_eq!(first.scanned, 1);
    assert_eq!(first.cached, 0);
    assert!(first.blocked.is_empty(), "{:?}", first.blocked);
    assert_eq!(first.plugins.len(), 1);
    let plugin = &first.plugins[0];
    assert_eq!(plugin.plugin_id, PLUGIN_ID);
    assert_eq!(plugin.name, PLUGIN_NAME);
    assert_eq!(plugin.vendor.as_deref(), Some("Beatlayer tests"));
    assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
    assert_eq!(plugin.format, clap());

    // Spec: plugin-scanning R2. An unchanged file is answered from the cache.
    let second = scanner.scan(clap(), &paths);
    assert_eq!(second.scanned, 0);
    assert_eq!(second.cached, 1);
    assert_eq!(second.plugins, first.plugins);

    // Spec: plugin-scanning R6. Rescan ignores the cache.
    let again = scanner.rescan(clap(), &paths);
    assert_eq!(again.scanned, 1);
    assert_eq!(again.cached, 0);
    assert_eq!(again.plugins, first.plugins);
}

// Spec: plugin-scanning R4.
#[test]
fn a_plugin_that_crashes_on_scan_is_blocklisted() {
    let scanner = scanner().with_env(TEST_SCAN_VAR, "crash");
    let report = scanner.scan(clap(), &[bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let BlockedPlugin { path, reason, .. } = &report.blocked[0];
    assert_eq!(path, bundle());
    assert!(
        reason.contains("crashed") || reason.contains("readable"),
        "{reason}"
    );
    // The scanner survived and remembers the blocklist.
    assert!(scanner.is_blocked(bundle()));
    assert_eq!(scanner.blocked().len(), 1);
}

// Spec: plugin-scanning R4.
#[test]
fn a_plugin_that_hangs_on_scan_times_out_and_is_blocklisted() {
    let scanner = scanner()
        .with_env(TEST_SCAN_VAR, "hang")
        .with_timeout(Duration::from_millis(500));
    let report = scanner.scan(clap(), &[bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(reason.contains("didn't answer"), "{reason}");
}

// Spec: plugin-scanning R3.
#[test]
fn a_file_that_isnt_a_plugin_is_blocklisted_with_why() {
    let dir = std::env::temp_dir().join(format!("bl-scan-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Not A Plugin.clap");
    std::fs::write(&path, b"not a clap entry").unwrap();
    let scanner = scanner();
    let report: ScanReport = scanner.scan(clap(), std::slice::from_ref(&path));
    assert_eq!(report.blocked.len(), 1);
    assert!(!report.blocked[0].reason.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R7.
#[test]
fn hidden_plugins_are_left_out_unless_asked_for() {
    let scanner = scanner();
    let paths = [bundle().to_path_buf()];
    let _ = scanner.scan(clap(), &paths);
    let plugin = scanner.plugins().into_iter().next().expect("scanned");
    let hidden = std::collections::BTreeSet::from([plugin.key()]);

    assert!(scanner.visible(false, &hidden, None).is_empty());
    assert_eq!(scanner.visible(true, &hidden, None).len(), 1);
    // The format preference keeps a plug-in that exists in only one format.
    assert_eq!(
        scanner
            .visible(false, &Default::default(), Some(PluginFormat::Clap))
            .len(),
        1
    );
    assert_eq!(
        scanner
            .visible(false, &Default::default(), Some(PluginFormat::Vst3))
            .len(),
        1
    );
}

// Spec: plugin-scanning R2. The cache is kept in a store file and read back.
#[test]
fn the_cache_round_trips_through_the_store_file() {
    let dir = std::env::temp_dir().join(format!("bl-scan-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("plugins.json");
    let paths = [bundle().to_path_buf()];

    let _ = Scanner::new()
        .with_program(SCANNER)
        .with_store(&store)
        .scan(clap(), &paths);
    let reopened = Scanner::new().with_program(SCANNER).with_store(&store);
    let report = reopened.scan(clap(), &paths);
    assert_eq!(report.cached, 1);
    assert_eq!(report.scanned, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R3. A folder of plug-ins is searched.
#[test]
fn a_folder_is_searched_for_plugin_files() {
    let dir = std::env::temp_dir().join(format!("bl-scan-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copied = dir.join("Test Gain Echo.clap");
    copy_bundle(bundle(), &copied);
    let scanner = scanner();
    let report = scanner.scan(clap(), &[PathBuf::from(&dir)]);
    assert_eq!(report.scanned, 1);
    assert_eq!(report.plugins.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R1; au-hosting's second part. Apple's built-in
// units, which need no third-party binaries, are found in
// /System/Library/Components and described by their component descriptions.
#[cfg(target_os = "macos")]
#[test]
fn a_scan_finds_apples_built_in_audio_units() {
    let scanner = scanner();
    let report = scanner.scan(
        PluginFormat::AudioUnit,
        &[PathBuf::from("/System/Library/Components")],
    );
    assert!(report.scanned > 0, "{report:?}");
    let found: Vec<&str> = report
        .plugins
        .iter()
        .map(|p| p.plugin_id.as_str())
        .collect();
    let delay = report
        .plugins
        .iter()
        .find(|p| p.plugin_id == "aufx/dely/appl")
        .unwrap_or_else(|| panic!("AUDelay among {found:?}"));
    assert_eq!(delay.name, "AUDelay");
    assert_eq!(delay.format, PluginFormat::AudioUnit);
    assert_eq!(delay.role, bl_model::DeviceRole::AudioEffect);
    assert!(delay.vendor.is_some(), "a vendor");
    assert_eq!(delay.key(), "au/aufx/dely/appl");
    assert!(
        delay.path.to_string_lossy().ends_with(".component"),
        "{:?}",
        delay.path
    );
}

/// Copies the test bundle to `to`, which is a folder on macOS and a file
/// elsewhere.
fn copy_bundle(from: &std::path::Path, to: &std::path::Path) {
    if from.is_dir() {
        let _ = std::fs::create_dir_all(to.join("Contents").join("MacOS"));
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                let _ = std::fs::copy(entry.path(), target);
            }
        }
    } else {
        std::fs::copy(from, to).unwrap();
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::create_dir_all(to);
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()));
        } else {
            let _ = std::fs::copy(entry.path(), to.join(entry.file_name()));
        }
    }
}

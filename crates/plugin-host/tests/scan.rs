//! The plug-in scanner (WP-2.9; the plug-in scanning spec). Each plug-in is
//! described in the `beatlayer-scan` child process, so a plug-in that crashes
//! or hangs during a scan is blocklisted instead of taking Beatlayer down,
//! and unchanged files are answered from a cache.

use std::path::PathBuf;
use std::time::Duration;

use bl_model::PluginFormat;
use bl_plugin_host::scan::{
    BlockedPlugin, ScanReport, Scanner, lv2_path_folders, plugin_files, system_paths,
};
use bl_test_clap::{PLUGIN_ID, PLUGIN_NAME, TEST_SCAN_VAR, bundle};
use bl_test_vst3 as test_vst3;

/// The scanner binary Cargo built beside these tests.
const SCANNER: &str = env!("CARGO_BIN_EXE_beatlayer-scan");

fn scanner() -> Scanner {
    Scanner::new().with_program(SCANNER)
}

fn clap() -> PluginFormat {
    PluginFormat::Clap
}

fn vst3() -> PluginFormat {
    PluginFormat::Vst3
}

fn lv2() -> PluginFormat {
    PluginFormat::Lv2
}

/// Whether the scanner's child can load lilv: on Linux always, elsewhere
/// when `BL_TEST_LILV` names a lilv library.
fn lilv() -> bool {
    cfg!(target_os = "linux") || std::env::var_os("BL_TEST_LILV").is_some()
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

// Spec: plugin-scanning R4. A reply bigger than the pipe's buffer must not
// look like a hang.
#[test]
fn a_reply_bigger_than_the_pipe_buffer_is_read() {
    let scanner = scanner()
        .with_env(TEST_SCAN_VAR, "big")
        .with_timeout(Duration::from_secs(20));
    let report = scanner.scan(clap(), &[bundle().to_path_buf()]);
    assert!(report.blocked.is_empty(), "{:?}", report.blocked);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].name.len(), 200_000);
}

// Spec: plugin-scanning R4. A plug-in's own text on standard output doesn't
// hide the reply.
#[test]
fn a_plugin_that_prints_a_banner_is_still_scanned() {
    let scanner = scanner().with_env(TEST_SCAN_VAR, "banner");
    let report = scanner.scan(clap(), &[bundle().to_path_buf()]);
    assert!(report.blocked.is_empty(), "{:?}", report.blocked);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].name, PLUGIN_NAME);
}

// Spec: plugin-scanning R4. A helper process the plug-in starts holds the
// reply pipe open only if the child lets it, and the scan doesn't wait for it.
#[cfg(unix)]
#[test]
fn a_plugin_that_leaves_a_helper_holding_stdout_is_still_scanned() {
    let scanner = scanner()
        .with_env(TEST_SCAN_VAR, "helper")
        .with_timeout(Duration::from_secs(2));
    let (tx, rx) = std::sync::mpsc::channel();
    let b = bundle().to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(scanner.scan(clap(), &[b]));
    });
    let report = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the scan blocked on a helper holding the reply pipe");
    assert!(report.blocked.is_empty(), "{:?}", report.blocked);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].name, PLUGIN_NAME);
}

// Spec: plugin-scanning R4, R6. A plain scan leaves a blocklisted plug-in
// alone until its file changes; Rescan tries it again.
#[test]
fn a_plain_scan_skips_a_blocklisted_plugin_that_hasnt_changed() {
    let scanner = scanner()
        .with_env(TEST_SCAN_VAR, "hang")
        .with_timeout(Duration::from_secs(1));
    let paths = [bundle().to_path_buf()];
    let first = scanner.scan(clap(), &paths);
    assert_eq!(first.blocked.len(), 1);
    let start = std::time::Instant::now();
    let second = scanner.scan(clap(), &paths);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(second.scanned, 0);
    assert_eq!(second.blocked.len(), 1);
    assert!(scanner.is_blocked(bundle()));
    let again = scanner.rescan(clap(), &paths);
    assert_eq!(again.scanned, 1);
}

// Spec: plugin-scanning R4. The blocklist's memory of a file survives a
// restart, and a changed file is tried again.
#[test]
fn a_blocklisted_plugin_stays_skipped_after_a_restart_until_it_changes() {
    let dir = std::env::temp_dir().join(format!("bl-scan-blocked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("scan.json");
    let copied = dir.join("Test Gain Echo.clap");
    copy_bundle(bundle(), &copied);
    let paths = [copied.clone()];
    let first = Scanner::new()
        .with_program(SCANNER)
        .with_env(TEST_SCAN_VAR, "crash")
        .with_store(&store)
        .scan(clap(), &paths);
    assert_eq!(first.blocked.len(), 1);
    // The app restarts: a new scanner reads the store, and the plug-in
    // would now load, but it isn't asked to.
    let restarted = scanner().with_store(&store);
    let second = restarted.scan(clap(), &paths);
    assert_eq!(second.scanned, 0);
    assert_eq!(second.blocked.len(), 1);
    // Touching the file makes it worth another try.
    let file = if copied.is_dir() {
        copied.join("Contents").join("Info.plist")
    } else {
        copied.clone()
    };
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let third = restarted.scan(clap(), &paths);
    assert_eq!(third.scanned, 1);
    assert!(third.blocked.is_empty(), "{:?}", third.blocked);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R2, R10. A bundle's files updated in place, which
// leaves its folder's own size and time as they were, are scanned again.
#[test]
fn a_vst3_bundle_updated_in_place_is_scanned_again() {
    let dir = std::env::temp_dir().join(format!("bl-scan-vst3-inplace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copied = dir.join("Test Gain Echo.vst3");
    copy_all(test_vst3::bundle(), &copied);
    let scanner = scanner();
    let paths = [copied.clone()];
    assert_eq!(scanner.scan(vst3(), &paths).scanned, 1);
    assert_eq!(scanner.scan(vst3(), &paths).cached, 1);

    let folder = std::fs::metadata(&copied).unwrap();
    let module = files_below(&copied.join("Contents"))
        .into_iter()
        .find(|file| file.extension().is_none_or(|e| e != "plist"))
        .expect("the module binary");
    std::fs::File::options()
        .write(true)
        .open(module)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let after = std::fs::metadata(&copied).unwrap();
    assert_eq!(folder.len(), after.len());
    assert_eq!(folder.modified().unwrap(), after.modified().unwrap());

    let again = scanner.scan(vst3(), &paths);
    assert_eq!(again.scanned, 1);
    assert_eq!(again.cached, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

fn files_below(folder: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        if entry.path().is_dir() {
            files.extend(files_below(&entry.path()));
        } else {
            files.push(entry.path());
        }
    }
    files
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
    // The child's own reason, not a guess that it crashed.
    let reason = &report.blocked[0].reason;
    assert!(!reason.is_empty());
    assert!(!reason.contains("crashed"), "{reason}");
    assert!(!reason.contains("exit status"), "{reason}");
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

// Spec: plugin-scanning R1, R2, R6. The scanner learns VST3 (WP-2.7).
#[test]
fn a_scan_describes_the_test_vst3_plugin_and_caches_it() {
    let scanner = scanner();
    let paths = [test_vst3::bundle().to_path_buf()];
    let first = scanner.scan(vst3(), &paths);
    assert_eq!(first.scanned, 1);
    assert_eq!(first.cached, 0);
    assert!(first.blocked.is_empty(), "{:?}", first.blocked);
    assert_eq!(first.plugins.len(), 1);
    let plugin = &first.plugins[0];
    assert_eq!(plugin.plugin_id, test_vst3::plugin_id());
    assert_eq!(plugin.name, test_vst3::PLUGIN_NAME);
    assert_eq!(plugin.vendor.as_deref(), Some("Beatlayer tests"));
    assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
    assert_eq!(plugin.format, vst3());

    // Spec: plugin-scanning R2. An unchanged file is answered from the cache.
    let second = scanner.scan(vst3(), &paths);
    assert_eq!(second.scanned, 0);
    assert_eq!(second.cached, 1);
    assert_eq!(second.plugins, first.plugins);

    // Spec: plugin-scanning R6. Rescan ignores the cache.
    let again = scanner.rescan(vst3(), &paths);
    assert_eq!(again.scanned, 1);
    assert_eq!(again.cached, 0);
    assert_eq!(again.plugins, first.plugins);
}

// Spec: plugin-scanning R1, R3. A folder of VST3 plug-ins is searched.
#[test]
fn a_folder_is_searched_for_vst3_plug_ins() {
    let dir = std::env::temp_dir().join(format!("bl-scan-vst3-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copied = dir.join("Test Gain Echo.vst3");
    copy_bundle(test_vst3::bundle(), &copied);
    let scanner = scanner();
    let report = scanner.scan(vst3(), &[PathBuf::from(&dir)]);
    assert_eq!(report.scanned, 1, "{:?}", report.blocked);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].plugin_id, test_vst3::plugin_id());
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R3, R4.
#[test]
fn a_vst3_module_that_hangs_on_scan_times_out_and_is_blocklisted() {
    let scanner = scanner()
        .with_env(test_vst3::TEST_SCAN_VAR, "hang")
        .with_timeout(Duration::from_millis(500));
    let report = scanner.scan(vst3(), &[test_vst3::bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(reason.contains("didn't answer"), "{reason}");
    assert!(scanner.is_blocked(test_vst3::bundle()));
}

// Spec: plugin-scanning R4.
#[test]
fn a_vst3_module_that_crashes_on_scan_is_blocklisted() {
    let scanner = scanner().with_env(test_vst3::TEST_SCAN_VAR, "crash");
    let report = scanner.scan(vst3(), &[test_vst3::bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(
        reason.contains("crashed") || reason.contains("readable"),
        "{reason}"
    );
}

// Spec: plugin-scanning R1, R10. The VST3 folders are the format's own
// locations for this platform.
#[test]
fn the_vst3_folders_follow_the_formats_locations() {
    let paths = system_paths(vst3());
    let ends_with = |name: &str| paths.iter().any(|p| p.ends_with(name));
    #[cfg(target_os = "macos")]
    {
        assert!(paths.contains(&PathBuf::from("/Library/Audio/Plug-Ins/VST3")));
        assert!(paths.contains(&PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3")));
        assert!(ends_with("Library/Audio/Plug-Ins/VST3"));
    }
    #[cfg(target_os = "linux")]
    {
        assert!(paths.contains(&PathBuf::from("/usr/lib/vst3")));
        assert!(paths.contains(&PathBuf::from("/usr/local/lib/vst3")));
        assert!(ends_with(".vst3"));
    }
    #[cfg(target_os = "windows")]
    {
        assert!(ends_with("VST3"));
    }
}

// Spec: plugin-scanning R10. A `.vst3` bundle folder counts as one file.
#[test]
fn vst3_bundles_are_taken_by_extension_and_name() {
    let dir = std::env::temp_dir().join(format!("bl-scan-vst3-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.vst3"), b"").unwrap();
    std::fs::write(dir.join("b.txt"), b"").unwrap();
    let bundle = dir.join("Synth.vst3");
    std::fs::create_dir_all(&bundle).unwrap();
    let files = plugin_files(vst3(), std::slice::from_ref(&dir));
    assert_eq!(files, vec![bundle, dir.join("a.vst3")]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Copies a file or a bundle folder, for a test that changes its copy.
fn copy_all(from: &std::path::Path, to: &std::path::Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            copy_all(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        std::fs::copy(from, to).unwrap();
    }
}

// Spec: plugin-scanning R13. A plug-in removed from a folder the scan
// searched is forgotten, so it leaves the browser.
#[test]
fn a_plugin_removed_from_a_scanned_folder_is_forgotten() {
    let dir = std::env::temp_dir().join(format!("bl-scan-removed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("Copied.clap");
    copy_all(bundle(), &copy);
    let scanner = scanner();
    let found = scanner.scan(clap(), std::slice::from_ref(&dir));
    assert_eq!(found.plugins.len(), 1, "{:?}", found.blocked);
    assert_eq!(scanner.plugins().len(), 1);
    if copy.is_dir() {
        std::fs::remove_dir_all(&copy).unwrap();
    } else {
        std::fs::remove_file(&copy).unwrap();
    }
    let again = scanner.scan(clap(), std::slice::from_ref(&dir));
    assert!(again.plugins.is_empty());
    assert!(scanner.plugins().is_empty(), "forgotten");
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R13. The scanner doesn't hold its cache while a
// plug-in is described, so what it knows can be read during a scan, as the
// browser does while the app's scan at start runs in the background.
#[test]
fn a_scan_answers_reads_while_it_runs() {
    let scanner = std::sync::Arc::new(
        scanner()
            .with_env(TEST_SCAN_VAR, "hang")
            .with_timeout(Duration::from_secs(3)),
    );
    let running = std::sync::Arc::clone(&scanner);
    let scan = std::thread::spawn(move || running.scan(clap(), &[bundle().to_path_buf()]));
    // Let the child start and hang.
    std::thread::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    assert!(scanner.plugins().is_empty());
    assert!(scanner.blocked().is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "reading waited for the scan: {:?}",
        started.elapsed()
    );
    assert!(
        !scan.is_finished(),
        "the hung plug-in is still being scanned"
    );
    let report = scan.join().unwrap();
    assert_eq!(report.blocked.len(), 1);
}

// Spec: plugin-scanning R1, R2, R6, R10. LV2 is scanned (WP-5.39): a bundle
// folder is one file holding its plug-ins.
#[test]
fn a_scan_describes_the_test_lv2_plugin_and_caches_it() {
    if !lilv() {
        return;
    }
    let scanner = scanner();
    let paths = [bl_test_lv2::bundle().to_path_buf()];
    let first = scanner.scan(lv2(), &paths);
    assert_eq!(first.scanned, 1);
    assert_eq!(first.cached, 0);
    assert!(first.blocked.is_empty(), "{:?}", first.blocked);
    assert_eq!(first.plugins.len(), 1);
    let plugin = &first.plugins[0];
    assert_eq!(plugin.plugin_id, bl_test_lv2::PLUGIN_URI);
    assert_eq!(plugin.name, bl_test_lv2::PLUGIN_NAME);
    assert_eq!(plugin.format, lv2());
    assert_eq!(plugin.role, bl_model::DeviceRole::AudioEffect);
    assert!(plugin.note_output);
    assert_eq!(plugin.path, bl_test_lv2::bundle());

    // An unchanged bundle is answered from the cache.
    let second = scanner.scan(lv2(), &paths);
    assert_eq!(second.scanned, 0);
    assert_eq!(second.cached, 1);
    assert_eq!(second.plugins, first.plugins);

    // Rescan ignores the cache.
    let again = scanner.rescan(lv2(), &paths);
    assert_eq!(again.scanned, 1);
    assert_eq!(again.cached, 0);
    assert_eq!(again.plugins, first.plugins);
}

// Spec: plugin-scanning R1, R10. A folder of LV2 bundles is searched, and a
// bundle edited in place is scanned again.
#[test]
fn a_folder_is_searched_for_lv2_bundles_and_an_edited_one_is_scanned_again() {
    if !lilv() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bl-scan-lv2-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copied = dir.join("test-gain-echo.lv2");
    copy_all(bl_test_lv2::bundle(), &copied);
    std::fs::write(dir.join("not-a-bundle.txt"), b"").unwrap();
    let scanner = scanner();
    let report = scanner.scan(lv2(), std::slice::from_ref(&dir));
    assert_eq!(report.scanned, 1, "{:?}", report.blocked);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].path, copied);
    assert_eq!(scanner.scan(lv2(), std::slice::from_ref(&dir)).cached, 1);

    // The manifest changes but the folder doesn't gain or lose a file: the
    // folder's own time wouldn't show it.
    let manifest = copied.join("manifest.ttl");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("{text}\n# edited\n")).unwrap();
    let again = scanner.scan(lv2(), std::slice::from_ref(&dir));
    assert_eq!(again.scanned, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R4, R5, R10. Instantiating a plug-in runs its code,
// here in the child process: one that crashes is blocklisted.
#[test]
fn an_lv2_plugin_that_crashes_on_scan_is_blocklisted() {
    if !lilv() {
        return;
    }
    let scanner = scanner().with_env("BL_TEST_LV2_SCAN", "crash");
    let report = scanner.scan(lv2(), &[bl_test_lv2::bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(
        reason.contains("crashed") || reason.contains("readable"),
        "{reason}"
    );
    assert!(scanner.is_blocked(bl_test_lv2::bundle()));
}

// Spec: plugin-scanning R3, R4, R5, R10.
#[test]
fn an_lv2_plugin_that_hangs_on_scan_times_out_and_is_blocklisted() {
    if !lilv() {
        return;
    }
    let scanner = scanner()
        .with_env("BL_TEST_LV2_SCAN", "hang")
        .with_timeout(Duration::from_millis(500));
    let report = scanner.scan(lv2(), &[bl_test_lv2::bundle().to_path_buf()]);
    assert!(report.plugins.is_empty());
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(reason.contains("didn't answer"), "{reason}");
}

// Spec: plugin-scanning R4. A folder that isn't a bundle of plug-ins is
// blocklisted with why.
#[test]
fn a_folder_that_isnt_an_lv2_bundle_is_blocklisted_with_why() {
    if !lilv() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bl-scan-lv2-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bundle = dir.join("empty.lv2");
    std::fs::create_dir_all(&bundle).unwrap();
    let report = scanner().scan(lv2(), std::slice::from_ref(&bundle));
    assert_eq!(report.blocked.len(), 1);
    let reason = &report.blocked[0].reason;
    assert!(!reason.is_empty());
    assert!(!reason.contains("crashed"), "{reason}");
    assert!(!reason.contains("exit status"), "{reason}");
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: plugin-scanning R10. `LV2_PATH` names the folders when it is set,
// `~` standing for the home folder, as lilv reads it.
#[test]
fn lv2_path_names_the_folders_searched() {
    let entries = std::env::join_paths(["/opt/lv2", "~/my-lv2"]).unwrap();
    assert_eq!(
        lv2_path_folders(&entries, Some(std::path::Path::new("/home/me"))),
        [PathBuf::from("/opt/lv2"), PathBuf::from("/home/me/my-lv2")]
    );
    // Without it, the standard folders.
    if std::env::var_os("LV2_PATH").is_none() {
        let paths = system_paths(lv2());
        assert!(paths.contains(&PathBuf::from("/usr/lib/lv2")));
        assert!(paths.contains(&PathBuf::from("/usr/local/lib/lv2")));
        assert!(paths.iter().any(|p| p.ends_with(".lv2")));
    }
}

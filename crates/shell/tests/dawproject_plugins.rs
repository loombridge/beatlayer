//! Plug-ins and saved sets through the DAWproject import (WP-6.36b; the
//! DAWproject spec, R46 to R48, R57): the test plug-ins, found by the
//! scanner, load from the files with the states they hold, and the import
//! maps a parameter's automation through the loaded plug-in. The real
//! plug-ins start on their own threads, so these tests wait for them.

use std::path::PathBuf;

use bl_shell_spec::dawproject::IMPORT_DAWPROJECT;

/// The folder the fixtures are in.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-shell-dawproject-plugins-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The shell's temporary folder, where an import writes what the file holds.
fn temporary(folder: &TempDir) -> PathBuf {
    folder.0.join("Temporary")
}

use bl_model::{AutomationTarget, DeviceId, DeviceKind, ParamId, PluginFormat};
use bl_shell::Shell;
use bl_shell::plugin_commands::{self as plugin_ids, PluginInfo};
use serde_json::json;

/// A shell with its engine running, whose scanner knows the test plug-ins in
/// `bundles`, and a scratch Temporary Folder.
fn shell_with_plugins(bundles: &[&std::path::Path], scratch: &TempDir) -> Shell {
    let mut shell = Shell::headless();
    shell.start_audio();
    let temporary = temporary(scratch);
    std::fs::create_dir_all(&temporary).expect("the temporary folder");
    shell
        .run(
            "settings.set_temporary_folder",
            json!({ "path": temporary.to_string_lossy() }),
        )
        .expect("the temporary folder is set");
    for bundle in bundles {
        shell
            .run(plugin_ids::SCAN, json!({ "paths": [bundle] }))
            .expect("the plug-in is scanned");
    }
    shell
}

/// Imports a fixture through the command.
fn import_fixture(shell: &mut Shell, name: &str) {
    let path = fixtures().join(format!("{name}.dawproject"));
    shell
        .run(IMPORT_DAWPROJECT, json!({ "path": path.to_string_lossy() }))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
}

/// The device of `format` in the open set.
fn plugin_device(shell: &Shell, format: PluginFormat) -> DeviceId {
    shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.devices_deep().collect::<Vec<_>>())
        .find(|device| matches!(&device.kind, DeviceKind::Plugin { format: f, .. } if *f == format))
        .map(|device| device.id)
        .expect("the set holds the plug-in")
}

/// Polls the shell, as the app's frames do, until `ready` holds of the
/// device's plug-in report.
fn wait(shell: &mut Shell, device: DeviceId, ready: impl Fn(&PluginInfo) -> bool) -> PluginInfo {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        shell.poll();
        if let Ok(value) = shell.run(plugin_ids::INFO, json!({ "device": device }))
            && let Ok(info) = serde_json::from_value::<PluginInfo>(value)
            && ready(&info)
        {
            return info;
        }
        assert!(
            std::time::Instant::now() < until,
            "the plug-in never got ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn value(info: &PluginInfo, param: u32) -> Option<f64> {
    info.values
        .iter()
        .find(|(id, _)| *id == ParamId(param))
        .map(|(_, v)| *v)
}

/// The test CLAP and VST3 plug-ins load from the files with the gain their
/// states hold (R46 to R48).
#[test]
fn plugins_found_on_this_machine_load_with_the_gain_their_states_hold() {
    let scratch = TempDir::new("plugins-load");
    let mut shell = shell_with_plugins(&[bl_test_clap::bundle(), bl_test_vst3::bundle()], &scratch);
    import_fixture(&mut shell, "bitwig-shaped");
    let clap = plugin_device(&shell, PluginFormat::Clap);
    let info = wait(&mut shell, clap, |info| info.active);
    assert_eq!(value(&info, bl_test_clap::GAIN), Some(0.5));
    assert_eq!(info.failure, None);

    shell.run("set.discard_changes", json!({})).ok();
    import_fixture(&mut shell, "cubase-shaped");
    let vst3 = plugin_device(&shell, PluginFormat::Vst3);
    let info = wait(&mut shell, vst3, |info| info.active);
    assert_eq!(value(&info, bl_test_vst3::GAIN), Some(1.5));
}

/// A plug-in parameter's `normalized` automation goes through the loaded
/// plug-in's own mapping (R57).
#[test]
fn a_plugin_parameters_normalized_automation_goes_through_its_mapping() {
    let scratch = TempDir::new("plugins-automation");
    let mut shell = shell_with_plugins(&[bl_test_vst3::bundle()], &scratch);
    import_fixture(&mut shell, "cubase-shaped");
    let document = shell.editor().document();
    let device = plugin_device(&shell, PluginFormat::Vst3);
    let instrument = document
        .tracks()
        .find(|track| track.name() == "Instrument")
        .expect("the track");
    let target = AutomationTarget::Device {
        device,
        param: ParamId(bl_test_vst3::GAIN),
    };
    let points: Vec<_> = instrument
        .automation_for(&target)
        .expect("the plug-in's envelope")
        .envelope
        .points()
        .map(|point| (point.at.get(), point.value))
        .collect();
    // 0 to 1 over the plug-in's gain, which runs from 0 to 2.
    assert_eq!(points, [(0, 0.5), (4 * 705_600, 1.5)]);
}

/// A plug-in that isn't installed is kept with its state, and saves it back
/// byte for byte (R46, R47).
#[test]
fn a_plugin_that_is_not_installed_saves_its_state_back_byte_for_byte() {
    let scratch = TempDir::new("plugins-missing");
    let mut shell = shell_with_plugins(&[], &scratch);
    import_fixture(&mut shell, "bitwig-shaped");
    let bytes = std::fs::read(fixtures().join("bitwig-shaped.dawproject")).unwrap();
    let container = bl_interop::dawproject::read_container(&bytes).unwrap();
    let state = container.read("plugins/state.clap-preset").unwrap();
    let device = plugin_device(&shell, PluginFormat::Clap);
    let held = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .plugin
        .clone()
        .unwrap();
    assert_eq!(held.data, state);
    let saved = scratch.0.join("Saved");
    std::fs::create_dir_all(&saved).unwrap();
    let set = saved.join("Kept.blset");
    let result = shell
        .run("set.save_as", json!({ "path": set.to_string_lossy() }))
        .expect("the set is saved");
    let path = result["path"].as_str().unwrap().to_owned();
    shell.run("set.new", json!({})).unwrap();
    shell.run("set.open", json!({ "path": path })).unwrap();
    let reopened = shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .plugin
        .clone()
        .unwrap();
    assert_eq!(reopened.data, state);
}

/// The imported set saves and reopens identically (acceptance).
#[test]
fn an_imported_set_saves_and_reopens_identically() {
    let scratch = TempDir::new("plugins-reopen");
    let mut shell = shell_with_plugins(&[], &scratch);
    for name in ["devices", "automation", "clips"] {
        shell.run("set.discard_changes", json!({})).ok();
        import_fixture(&mut shell, name);
        let before = shell.editor().document().clone();
        let saved = scratch.0.join(format!("Saved-{name}"));
        std::fs::create_dir_all(&saved).unwrap();
        let result = shell
            .run(
                "set.save_as",
                json!({ "path": saved.join("Set.blset").to_string_lossy() }),
            )
            .expect("the set is saved");
        let path = result["path"].as_str().unwrap().to_owned();
        shell.run("set.new", json!({})).unwrap();
        shell.run("set.open", json!({ "path": path })).unwrap();
        assert_eq!(shell.editor().document(), &before, "{name} changed");
    }
}

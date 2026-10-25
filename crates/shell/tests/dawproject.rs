//! Importing a DAWproject file as a new set (WP-6.36), as the behaviour
//! behind `set.import_dawproject` and `set.import_report`.

use std::path::PathBuf;

use bl_model::TrackId;
use bl_shell::dawproject;
use bl_shell::testing::rig;
use bl_shell_spec::dawproject::{
    IMPORT_DAWPROJECT, IMPORT_REPORT, ImportDawProjectArgs, ImportDawProjectResult, ImportReport,
};

/// The folder the fixtures are in.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-shell-dawproject-{name}"));
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

/// An import opens the file as a new set, with the file's name and the
/// report (R10, R11, R12).
#[test]
fn an_import_opens_the_file_as_a_new_set() {
    let (mut shell, _control) = rig();
    let path = fixtures().join("minimal.dawproject");
    let result = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    let result: ImportDawProjectResult = serde_json::from_value(result).expect("the result");
    assert_eq!(result.name, "minimal");
    assert!(result.report.entries.is_empty(), "{:?}", result.report);
    // The set is the new one, and it is unsaved.
    assert_eq!(shell.set_name(), "minimal");
    assert!(shell.editor().document().track_count() > 1);
}

/// The import is refused while the open set has unsaved changes, and the set
/// is left as it was (R10).
#[test]
fn an_import_with_unsaved_changes_is_refused() {
    let (mut shell, _control) = rig();
    let before = shell.editor().document().clone();
    let id: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            serde_json::json!({ "id": id, "kind": "midi", "name": "Added", "index": 0 }),
        )
        .expect("a track is made");
    let path = fixtures().join("minimal.dawproject");
    let error = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect_err("the import is refused");
    assert!(error.to_string().contains("unsaved changes"), "{error}");
    // The set is as it was.
    assert_eq!(
        shell.editor().document().track_count(),
        before.track_count() + 1
    );
}

/// A file that isn't a DAWproject file leaves the open set as it was
/// (R10).
#[test]
fn a_failed_import_leaves_the_open_set_unchanged() {
    let (mut shell, _control) = rig();
    let before = shell.editor().document().clone();
    let folder = TempDir::new("failed");
    let path = folder.0.join("broken.dawproject");
    std::fs::write(&path, b"this is not a DAWproject file").expect("the file is written");
    let error = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect_err("the import is refused");
    assert!(error.to_string().contains("DAWproject"), "{error}");
    assert_eq!(shell.editor().document(), &before);
    assert_eq!(shell.set_name(), "Untitled");
}

/// The report is given again by `set.import_report` while the set the import
/// made is open (R12).
#[test]
fn the_report_is_read_back_while_the_imported_set_is_open() {
    let (mut shell, _control) = rig();
    let _folder = TempDir::new("report");
    let path = fixtures().join("clips.dawproject");
    let imported = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    let imported: ImportDawProjectResult = serde_json::from_value(imported).expect("the result");
    let report = shell
        .run(IMPORT_REPORT, serde_json::json!({}))
        .expect("the report is read");
    let report: ImportReport = serde_json::from_value(report).expect("the report");
    assert_eq!(report, imported.report);
    assert!(!report.entries.is_empty());

    // Another set opening takes the report and the file's name away (R11,
    // R12).
    shell
        .run("set.new", serde_json::json!({}))
        .expect("a new set opens");
    assert_eq!(shell.set_name(), "Untitled");
    let report = shell
        .run(IMPORT_REPORT, serde_json::json!({}))
        .expect("the report is read");
    let report: ImportReport = serde_json::from_value(report).expect("the report");
    assert!(report.entries.is_empty());
}

/// The files the file holds are written into the Temporary Folder, and the
/// first save moves them into the project's `Samples/Imported` (R39).
#[test]
fn the_files_the_file_holds_are_written_and_moved_on_the_first_save() {
    let (mut shell, _control) = rig();
    let folder = TempDir::new("media");
    let temporary = temporary(&folder);
    std::fs::create_dir_all(&temporary).expect("the temporary folder");
    shell
        .run(
            "settings.set_temporary_folder",
            serde_json::json!({ "path": temporary.to_string_lossy() }),
        )
        .expect("the temporary folder is set");
    let path = fixtures().join("media.dawproject");
    shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    // The file the container held is in the Temporary Folder, under
    // `Imported/<the file's name without .dawproject>/`.
    let extracted = temporary.join("Imported/media/audio/drum.wav");
    assert!(extracted.is_file(), "{} is missing", extracted.display());

    // The first save asks where to save the set.
    let saved = folder.0.join("Saved");
    std::fs::create_dir_all(&saved).expect("a folder to save in");
    let set = saved.join("Saved.blset");
    let result = shell
        .run(
            "set.save_as",
            serde_json::json!({ "path": set.to_string_lossy() }),
        )
        .expect("the set is saved");
    let result: bl_shell_spec::commands::SetResult =
        serde_json::from_value(result).expect("the result");
    // The file has moved into the project's `Samples/Imported`.
    let project = result
        .path
        .as_ref()
        .and_then(|path| PathBuf::from(path).parent().map(PathBuf::from))
        .expect("the set's file");
    let moved = project.join("Samples/Imported/drum.wav");
    assert!(moved.is_file(), "{} is missing", moved.display());
    assert!(!extracted.exists());
}

/// The arguments of the import command are what the spec says.
#[test]
fn the_arguments_are_the_path_of_the_file() {
    let args: ImportDawProjectArgs =
        serde_json::from_value(serde_json::json!({ "path": "/a/b.dawproject" }))
            .expect("the arguments");
    assert_eq!(args.path, "/a/b.dawproject");
    assert!(serde_json::from_value::<ImportDawProjectArgs>(serde_json::json!({})).is_err());
}

/// The import's folder is under the Temporary Folder, named after the file
/// (R39).
#[test]
fn the_import_folder_is_named_after_the_file() {
    let folder = dawproject::import_folder(
        std::path::Path::new("/tmp/Temporary"),
        std::path::Path::new("/music/Song.dawproject"),
    );
    assert_eq!(folder, PathBuf::from("/tmp/Temporary/Imported/Song"));
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
    let state = container.files["plugins/state.clap-preset"].clone();
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

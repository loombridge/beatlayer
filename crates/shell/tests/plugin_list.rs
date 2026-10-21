//! The list of a set's plug-ins (IMP-29, WP-6.41; the plug-in list spec):
//! `plugin.list` names every plug-in the set uses with the tracks it is on
//! and the preset names its devices hold, a preset loaded on a device gives
//! it its name, and `plugin.export_list` writes the list to a text file.

use bl_model::{Command, Device, DeviceKind, PluginFormat, TrackId};
use bl_shell::Shell;
use bl_shell::plugin_commands::{self as ids, SetPluginList};
use serde_json::json;

/// The scanner's child process is built beside these tests; a run that
/// didn't build it can't scan.
fn ready() -> bool {
    bl_plugin_host::scan::scan_program().is_some()
}

/// The first two tracks of the set, which the plug-in goes on.
fn two_tracks(shell: &Shell) -> Vec<TrackId> {
    shell
        .editor()
        .document()
        .tracks()
        .take(2)
        .map(bl_model::Track::id)
        .collect()
}

/// A device for a plug-in this computer doesn't have, in the set.
fn insert_missing(shell: &mut Shell, track: TrackId) {
    shell.edit(|editor| {
        let id = editor.new_id();
        let device = Device::new(
            id,
            "Missing Plug-In",
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                plugin_id: "beatlayer.tests.missing".to_owned(),
            },
        );
        let index = editor.document().track(track).unwrap().devices().len();
        editor
            .apply(Command::InsertDevice {
                track,
                index,
                device,
            })
            .unwrap();
    });
}

// Spec: plugin-list R2, R5, R11; plugin-scanning R8.
#[test]
fn the_set_list_names_the_plug_ins_with_their_tracks_and_preset_names() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    // The scanner knows the plug-in, so it isn't missing.
    shell
        .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
        .unwrap();
    let tracks = two_tracks(&shell);
    assert_eq!(tracks.len(), 2, "the set has two tracks to use");
    let mut devices = Vec::new();
    for track in &tracks {
        let loaded: bl_shell::plugin_commands::PluginLoaded = serde_json::from_value(
            shell
                .run(ids::LOAD, json!({ "path": bundle, "track": track }))
                .unwrap(),
        )
        .unwrap();
        devices.push(loaded.device);
    }
    // A plug-in this computer doesn't have, on the first track.
    insert_missing(&mut shell, tracks[0]);

    // A preset loaded on a device gives it the preset's name.
    let library = std::env::temp_dir().join(format!("bl-plugin-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&library);
    let preset = library.join("Warm Pad.blpreset");
    shell
        .run(
            "device.save_preset",
            json!({ "device": devices[0], "path": preset.to_string_lossy() }),
        )
        .unwrap();
    shell
        .run(
            "device.load_preset",
            json!({ "device": devices[0], "path": preset.to_string_lossy() }),
        )
        .unwrap();

    let list: SetPluginList =
        serde_json::from_value(shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins.len(), 2, "{list:?}");
    // The rows are in the list's order, by name.
    let found = list
        .plugins
        .iter()
        .find(|plugin| !plugin.missing)
        .expect("the plug-in the scanner knows");
    assert_eq!(found.plugin_id, bl_test_clap::PLUGIN_ID);
    assert_eq!(found.tracks, tracks, "both tracks");
    assert_eq!(found.devices, devices, "both devices");
    assert_eq!(found.preset_names, ["Warm Pad"], "the preset's name");
    assert!(found.blocked.is_none());
    let missing = list
        .plugins
        .iter()
        .find(|plugin| plugin.missing)
        .expect("the plug-in this computer doesn't have");
    assert_eq!(missing.tracks, [tracks[0]]);
    assert!(
        missing.preset_names.is_empty(),
        "no preset was loaded on it"
    );
    let _ = std::fs::remove_dir_all(&library);
}

// Spec: plugin-list R5, R7.
#[test]
fn a_preset_name_survives_the_set_being_saved_and_opened_again() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    let tracks = two_tracks(&shell);
    let loaded: bl_shell::plugin_commands::PluginLoaded = serde_json::from_value(
        shell
            .run(ids::LOAD, json!({ "path": bundle, "track": tracks[0] }))
            .unwrap(),
    )
    .unwrap();
    let library =
        std::env::temp_dir().join(format!("bl-plugin-list-preset-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&library);
    let preset = library.join("Warm Pad.blpreset");
    shell
        .run(
            "device.save_preset",
            json!({ "device": loaded.device, "path": preset.to_string_lossy() }),
        )
        .unwrap();
    shell
        .run(
            "device.load_preset",
            json!({ "device": loaded.device, "path": preset.to_string_lossy() }),
        )
        .unwrap();
    let list: SetPluginList =
        serde_json::from_value(shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins[0].preset_names, ["Warm Pad"]);

    // The set keeps it: saved and opened again, the device holds the
    // preset's name.
    let folder = std::env::temp_dir().join(format!("bl-plugin-list-set-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let saved: bl_shell_spec::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": folder.join("Demo.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    // The set goes in a project folder of its own.
    let set = std::path::PathBuf::from(saved.path.expect("the set was saved"));
    let mut reopened = Shell::headless();
    reopened
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    let list: SetPluginList =
        serde_json::from_value(reopened.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins.len(), 1, "{list:?}");
    assert_eq!(list.plugins[0].preset_names, ["Warm Pad"]);

    // A set saved before this package has no preset name: the device
    // loads with none.
    let bytes = std::fs::read(&set).unwrap();
    let (mut project, info) =
        bl_project::format::decode(&bytes, &bl_project::LoadOptions::default()).unwrap();
    let mut editor = project.editor();
    let device = editor
        .document()
        .tracks()
        .next()
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .id;
    editor.amend_device_preset_name(device, None).unwrap();
    project.document = editor.document().clone();
    project.ids = editor.ids();
    let older = folder.join("Older.blset");
    std::fs::write(
        &older,
        bl_project::format::encode(&project, info.revision + 1).unwrap(),
    )
    .unwrap();
    let mut older_shell = Shell::headless();
    older_shell
        .run("set.open", json!({ "path": older.to_string_lossy() }))
        .unwrap();
    let list: SetPluginList =
        serde_json::from_value(older_shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins.len(), 1, "{list:?}");
    assert!(
        list.plugins[0].preset_names.is_empty(),
        "a set saved before this package has no names: {:?}",
        list.plugins[0].preset_names
    );
    let _ = std::fs::remove_dir_all(&library);
    let _ = std::fs::remove_dir_all(&folder);
}

// Spec: plugin-list R5. An Audio Unit reports the preset it holds, and the
// device keeps its name (macOS only).
#[cfg(target_os = "macos")]
#[test]
fn an_audio_unit_keeps_the_name_of_the_preset_it_reports() {
    let mut shell = Shell::headless();
    let track = shell.editor().document().tracks().next().unwrap().id();
    let loaded: bl_shell::plugin_commands::PluginLoaded = serde_json::from_value(
        shell
            .run(
                "plugin.load_au",
                json!({
                    "type": "aufx",
                    "subtype": "dely",
                    "manufacturer": "appl",
                    "track": track,
                }),
            )
            .expect("Apple's AUDelay loads"),
    )
    .unwrap();
    let name = shell
        .editor()
        .document()
        .device(loaded.device)
        .unwrap()
        .preset_name
        .clone();
    assert!(
        name.is_some(),
        "the unit reports the preset it holds: {name:?}"
    );
    let list: SetPluginList =
        serde_json::from_value(shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(
        list.plugins[0].preset_names,
        name.into_iter().collect::<Vec<String>>(),
        "the list reports it"
    );
}

// Spec: plugin-list R1, R3, R10.
#[test]
fn the_view_state_says_whether_the_list_shows_and_shows_only_the_missing() {
    let mut shell = Shell::headless();
    let state: bl_shell_spec::commands::ViewState =
        serde_json::from_value(shell.run("view.state", json!({})).unwrap()).unwrap();
    assert!(!state.plugin_list.visible, "the list is hidden at first");
    assert!(!state.plugin_list.only_missing);

    shell
        .run("view.toggle_plugin_list", json!({}))
        .expect("the list opens");
    let state: bl_shell_spec::commands::ViewState =
        serde_json::from_value(shell.run("view.state", json!({})).unwrap()).unwrap();
    assert!(state.plugin_list.visible, "the list shows");

    shell
        .run(
            "view.set_plugin_list_only_missing",
            json!({ "only_missing": true }),
        )
        .expect("Show Only Missing turns on");
    let state: bl_shell_spec::commands::ViewState =
        serde_json::from_value(shell.run("view.state", json!({})).unwrap()).unwrap();
    assert!(state.plugin_list.only_missing);

    // Show Only Missing is off each time the list opens.
    shell
        .run("view.toggle_plugin_list", json!({}))
        .expect("the list closes");
    shell
        .run("view.toggle_plugin_list", json!({}))
        .expect("the list opens again");
    let state: bl_shell_spec::commands::ViewState =
        serde_json::from_value(shell.run("view.state", json!({})).unwrap()).unwrap();
    assert!(state.plugin_list.visible);
    assert!(!state.plugin_list.only_missing);
}

// Spec: plugin-list R7.
#[test]
fn the_export_writes_the_list_as_tab_separated_text() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    shell
        .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
        .unwrap();
    let tracks = two_tracks(&shell);
    shell
        .run(ids::LOAD, json!({ "path": bundle, "track": tracks[0] }))
        .unwrap();
    let folder = std::env::temp_dir().join(format!("bl-plugin-list-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("Plug-Ins.txt");
    let exported: bl_shell::plugin_commands::ExportedList = serde_json::from_value(
        shell
            .run(ids::EXPORT_LIST, json!({ "path": path.to_string_lossy() }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(exported.path, path.to_string_lossy());
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    // The first line has the set's name and the date.
    let first: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(first[0], "Untitled", "{text}");
    assert_eq!(first[1].len(), 10, "the date: {text}");
    // The header line names the columns.
    assert_eq!(
        lines[1],
        "Name\tFormat\tVendor\tVersion\tPresets\tTracks\tMissing"
    );
    // The plug-in's own line: its name, its format, and the track it's on.
    let row: Vec<&str> = lines[2].split('\t').collect();
    assert_eq!(row.len(), 7, "{text}");
    assert_eq!(row[1], "CLAP", "{text}");
    let track_name = shell
        .editor()
        .document()
        .track(tracks[0])
        .unwrap()
        .name()
        .to_owned();
    assert_eq!(row[5], track_name, "{text}");
    assert_eq!(row[6], "", "this computer has it: {text}");
    let _ = std::fs::remove_dir_all(&folder);
}

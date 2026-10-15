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
use bl_test_clap::{GAIN, LAST_NOTE, PLUGIN_ID, PLUGIN_NAME};
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

/// Loads the test plug-in onto `track` and returns its device.
fn load(shell: &mut Shell, track: bl_model::TrackId) -> DeviceId {
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(
                ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    loaded.device
}

/// `plugin.info`'s report for `device`.
fn info(shell: &mut Shell, device: DeviceId) -> PluginInfo {
    let value = shell.run(ids::INFO, json!({ "device": device })).unwrap();
    serde_json::from_value(value).unwrap()
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
    assert_eq!(shell.editor().undo_history().next(), Some("Insert Device"));
    let device = loaded.device;

    // The Device View describes it, and agents read its parameters.
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    let frame = view.devices.iter().find(|d| d.id == device).unwrap();
    assert!(frame.selected);
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 6);
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

// Spec: clap-hosting R6
#[test]
fn a_change_a_plugin_makes_itself_is_one_undo_step() {
    use bl_engine::{Block, NodeConfig, NoteBuffer, NoteEvent, NoteKind, TransportInfo};

    let mut shell = Shell::headless();
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
    // Run the plug-in's node here, as the engine would, and play it a
    // note: it sets its own Last Note, in a gesture, as its editor would.
    let device = shell
        .editor()
        .document()
        .device(loaded.device)
        .unwrap()
        .clone();
    let config = NodeConfig {
        sample_rate: bl_core::time::SampleRate::HZ_48000,
        max_frames: 64,
        channels: 2,
    };
    let mut node = bl_engine::NodeFactory::create(&shell.plugins().factory(), &device, &config)
        .expect("a node");
    let mut data = vec![0.0f32; 128];
    let mut out = NoteBuffer::with_capacity(8);
    let notes = [NoteEvent {
        offset: 3,
        kind: NoteKind::On,
        channel: 0,
        key: 67,
        velocity: 1.0,
        note_id: None,
    }];
    let mut block = Block::new(
        &mut data,
        2,
        64,
        64,
        &[],
        TransportInfo::default(),
        bl_core::time::SampleRate::HZ_48000,
    )
    .with_notes(&notes, &mut out);
    node.process(&mut block);

    let steps = shell.editor().undo_history().len();
    assert!(shell.poll());
    assert_eq!(
        shell
            .editor()
            .document()
            .device(loaded.device)
            .unwrap()
            .params
            .get(&ParamId(bl_test_clap::LAST_NOTE)),
        Some(&67.0)
    );
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Last Note\"")
    );
}

// Spec: device-chains R31, R33. View ▸ Plug-In Windows (Cmd-Opt-P)
// hides and shows a plug-in's editor window as it hides and shows a
// floating device's: hiding is not closing, it isn't an undo step, it
// changes nothing in the set, and the menu item's tick and enabled
// state account for it.
#[test]
fn the_plug_in_windows_toggle_hides_and_shows_a_plugins_editor() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    assert!(wait(&mut shell, device, |i| i.active).has_window);

    // Disabled with no plug-in window open, as Live's is.
    assert!(!shell.can_invoke("view.toggle_plug_in_windows", &json!({})));

    // Opening its editor enables the menu item, ticked while shown.
    let open: PluginInfo = serde_json::from_value(
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert!(open.window_open && open.window_shown);
    assert!(shell.can_invoke("view.toggle_plug_in_windows", &json!({})));
    assert_eq!(
        shell.switch_state("view.toggle_plug_in_windows"),
        Some(true)
    );

    let steps = shell.editor().undo_history().len();
    let dirty = shell.is_dirty();
    // Cmd-Option-P (Ctrl-Alt-P) and View ▸ Plug-In Windows.
    let reply = shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert_eq!(reply, json!({ "on": false }));
    let hidden = info(&mut shell, device);
    assert!(hidden.window_open, "hiding is not closing");
    assert!(!hidden.window_shown, "hidden");
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "nothing in the set changed");
    assert_eq!(
        shell.switch_state("view.toggle_plug_in_windows"),
        Some(false)
    );

    // Shown again.
    let reply = shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert_eq!(reply, json!({ "on": true }));
    assert!(info(&mut shell, device).window_shown, "shown again");

    // Closing the window is still closing: the item is disabled again.
    shell
        .run(
            ids::SHOW_WINDOW,
            json!({ "device": device, "shown": false }),
        )
        .unwrap();
    assert!(!shell.can_invoke("view.toggle_plug_in_windows", &json!({})));
}

// Spec: device-chains R31. A plug-in's editor follows Auto-Hide Plug-In
// Windows: hidden while its track isn't selected, shown again when it
// is; off, it stays shown whatever track is selected.
#[test]
fn a_plugins_editor_follows_auto_hide_plug_in_windows() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    wait(&mut shell, device, |i| i.active);
    assert!(shell.auto_hide_plug_in_windows(), "on in a new install");
    shell
        .run(ids::SHOW_WINDOW, json!({ "device": device }))
        .unwrap();
    assert!(
        info(&mut shell, device).window_shown,
        "shown with its track"
    );

    // Another track selected: hidden, not closed.
    let other = shell.editor().document().top_level().nth(3).unwrap();
    assert_ne!(other, track);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [other] }))
        .unwrap();
    shell.poll();
    let hidden = info(&mut shell, device);
    assert!(hidden.window_open, "hiding is not closing");
    assert!(
        !hidden.window_shown,
        "hidden while another track is selected"
    );

    // Its track again: shown again.
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.poll();
    assert!(info(&mut shell, device).window_shown, "back with its track");

    // Off, it stays shown whatever track is selected.
    shell
        .run(
            "settings.set_auto_hide_plug_in_windows",
            json!({ "on": false }),
        )
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [other] }))
        .unwrap();
    shell.poll();
    assert!(info(&mut shell, device).window_shown);

    // Opened while its track isn't the selected one: hidden at once.
    shell
        .run(
            "settings.set_auto_hide_plug_in_windows",
            json!({ "on": true }),
        )
        .unwrap();
    shell
        .run(
            ids::SHOW_WINDOW,
            json!({ "device": device, "shown": false }),
        )
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [other] }))
        .unwrap();
    shell
        .run(ids::SHOW_WINDOW, json!({ "device": device }))
        .unwrap();
    let opened = info(&mut shell, device);
    assert!(
        opened.window_open && !opened.window_shown,
        "auto-hidden at once"
    );
}

// Spec: clap-hosting R13, R16; WP-2.10. A plug-in's editor window is part
// of the window layer: where it sits and how big it is are saved with the
// set beside the document, so it reopens where it was left, and moving or
// resizing it is never an undo step.
#[test]
fn a_plugin_windows_place_survives_a_set_round_trip() {
    let scratch =
        std::env::temp_dir().join(format!("bl-shell-plugin-window-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    wait(&mut shell, device, |i| i.active);
    shell
        .run(ids::SHOW_WINDOW, json!({ "device": device }))
        .unwrap();

    let frame = bl_model::WindowFrame {
        x: 120.0,
        y: 80.0,
        width: 500.0,
        height: 400.0,
    };
    let steps = shell.editor().undo_history().len();
    let placed: PluginInfo = serde_json::from_value(
        shell
            .run(ids::SET_WINDOW, json!({ "device": device, "frame": frame }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(placed.window, Some(frame));
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");

    // A frame that isn't finite, or whose size isn't above 0, is refused.
    assert!(
        shell
            .run(
                ids::SET_WINDOW,
                json!({ "device": device, "frame": { "x": 0.0, "y": 0.0, "width": -1.0, "height": 1.0 } }),
            )
            .is_err()
    );

    // Saved and opened again, the window is where it was left.
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": scratch.join("Windows.blset") }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    let reopened = wait(&mut shell, device, |i| i.active);
    assert_eq!(reopened.window, Some(frame));

    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: clap-hosting R16; the plug-in device panel (WP-2.10, D19, IMP-03).
// A plug-in device's panel is eight knobs mapped to the plug-in's first
// parameters, remappable with `plugin.set_panel_param`, and the mapping is
// remembered per plug-in.
#[test]
fn the_panel_maps_the_plugins_parameters_and_a_remap_is_remembered_per_plugin() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    wait(&mut shell, device, |i| i.active);
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();

    let panel = |shell: &mut Shell| {
        let view: DeviceViewState =
            serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
        view.devices
            .iter()
            .find(|d| d.id == device)
            .unwrap()
            .panel
            .clone()
            .unwrap()
    };

    let first = panel(&mut shell);
    assert!(first.plugin, "a plug-in device's panel");
    let names: Vec<&str> = first
        .description
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Gain",
            "Latency",
            "Last Note",
            "Editor Scale",
            "Editor Width",
            "Editor Height"
        ]
    );

    // Remap the first knob to the plug-in's third parameter.
    let remapped: DeviceViewState = serde_json::from_value(
        shell
            .run(
                ids::SET_PANEL_PARAM,
                json!({ "device": device, "index": 0, "param": LAST_NOTE }),
            )
            .unwrap(),
    )
    .unwrap();
    let after = remapped
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(after.description.params[0].name, "Last Note");
    assert_eq!(
        shell
            .preferences()
            .plugin_panels
            .get(&format!("clap/{PLUGIN_ID}"))
            .and_then(|m| m.get(&0)),
        Some(&ParamId(LAST_NOTE)),
        "recorded per plug-in"
    );

    // Another device of the same plug-in keeps the mapping.
    let other = load(&mut shell, track);
    shell
        .run("device.select", json!({ "device": other }))
        .unwrap();
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    let other_panel = view
        .devices
        .iter()
        .find(|d| d.id == other)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(other_panel.description.params[0].name, "Last Note");

    // Null restores the knob to the plug-in's parameter at that place.
    shell
        .run(
            ids::SET_PANEL_PARAM,
            json!({ "device": other, "index": 0, "param": null }),
        )
        .unwrap();
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    let restored = view
        .devices
        .iter()
        .find(|d| d.id == other)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(restored.description.params[0].name, "Gain");
}

// Spec: device-chains R31, R33; WP-2.10. View ▸ Plug-In Windows hides and
// shows every open plug-in window, not just the selected device's.
#[test]
fn plug_in_windows_hides_and_shows_every_editor() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let a = load(&mut shell, track);
    let b = load(&mut shell, track);
    wait(&mut shell, a, |i| i.active);
    wait(&mut shell, b, |i| i.active);
    shell.run(ids::SHOW_WINDOW, json!({ "device": a })).unwrap();
    shell.run(ids::SHOW_WINDOW, json!({ "device": b })).unwrap();
    assert!(info(&mut shell, a).window_shown && info(&mut shell, b).window_shown);

    shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert!(
        !info(&mut shell, a).window_shown && !info(&mut shell, b).window_shown,
        "both hidden"
    );

    shell
        .invoke("view.toggle_plug_in_windows", json!({}))
        .unwrap();
    assert!(
        info(&mut shell, a).window_shown && info(&mut shell, b).window_shown,
        "both shown again"
    );
}

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

/// `device.parameters`, each marked configured or not.
fn parameters(shell: &mut Shell, device: DeviceId) -> DeviceParameters {
    serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

/// The plug-in's configured parameter numbers, as `device.parameters` lists
/// them (the plug-in's own order).
fn configured(shell: &mut Shell, device: DeviceId) -> Vec<u32> {
    parameters(shell, device)
        .params
        .iter()
        .filter(|p| p.configured)
        .map(|p| p.descriptor.id.0)
        .collect()
}

/// The panel's knob parameter numbers, in the configured list's order.
fn panel_order(shell: &mut Shell, device: DeviceId) -> Vec<u32> {
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    view.devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap()
        .description
        .params
        .iter()
        .map(|p| p.id.0)
        .collect()
}

/// Plays `device` a note, as the engine would, so its editor would have set
/// its Last Note: what a person moving a parameter in the plug-in's window
/// does.
fn plug_in_moves_last_note(shell: &mut Shell, device: DeviceId) {
    use bl_engine::{Block, NodeConfig, NoteBuffer, NoteEvent, NoteKind, TransportInfo};

    let device = shell.editor().document().device(device).unwrap().clone();
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
    assert!(shell.poll());
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
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 8);
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
            "Editor Height",
            "Depth",
            "Depth Modulation"
        ]
    );

    // Scale the first knob to the plug-in's third parameter.
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

// Spec: device-chains R40; IMP-25. Close All Plug-in Windows closes every
// open editor window at once; it changes nothing in the set and is never an
// undo step.
#[test]
fn close_all_plug_in_windows_closes_every_editor_without_an_undo_step() {
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
    assert!(info(&mut shell, a).window_open && info(&mut shell, b).window_open);

    assert!(shell.can_invoke("view.close_plug_in_windows", &json!({})));
    let before = shell.editor().document().clone();
    let steps = shell.editor().undo_history().len();
    let closed: bl_shell_spec::commands::ClosedPlugInWindows = serde_json::from_value(
        shell
            .invoke("view.close_plug_in_windows", json!({}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(closed.closed, 2, "both editors closed");
    assert!(!info(&mut shell, a).window_open && !info(&mut shell, b).window_open);
    assert_eq!(*shell.editor().document(), before, "the set is unchanged");
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "closing a window is no undo step"
    );

    // With no editor open the item is disabled: it never changes a floating
    // native device window.
    assert!(!shell.can_invoke("view.close_plug_in_windows", &json!({})));
}

// Spec: vst3-hosting R1, R3, R4, R9. A VST3 plug-in loads by path through
// the commands as an agent runs them, the Device View describes it, and it
// processes a block through the shell's host with its parameters.
#[test]
fn a_vst3_plugin_loads_runs_and_processes_through_the_commands() {
    use bl_core::time::SampleRate;
    use bl_engine::{Block, NodeConfig, NodeFactory, NoteBuffer, TransportInfo};

    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(
                ids::LOAD,
                json!({ "path": bl_test_vst3::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (loaded.plugin.as_str(), loaded.name.as_str()),
        (
            bl_test_vst3::plugin_id().as_str(),
            bl_test_vst3::PLUGIN_NAME
        )
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
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 3);
    let params: DeviceParameters = serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(params.params[0].descriptor.name, "Gain");

    // The engine runs it: a block comes back with the gain applied, and
    // `plugin.info` says it's active with its one parameter.
    let device = shell.editor().document().device(device).unwrap().clone();
    let config = NodeConfig {
        sample_rate: SampleRate::HZ_48000,
        max_frames: 64,
        channels: 2,
    };
    let mut node = NodeFactory::create(&shell.vst3().factory(), &device, &config)
        .expect("a node for a VST3 device");
    let mut data = vec![0.5f32; 128];
    let mut out = NoteBuffer::with_capacity(8);
    {
        let mut block = Block::new(
            &mut data,
            2,
            64,
            64,
            &[],
            TransportInfo::default(),
            SampleRate::HZ_48000,
        )
        .with_notes(&[], &mut out);
        node.process(&mut block);
    }
    assert!(data.iter().all(|s| *s == 0.5), "the gain of 1: {data:?}");
    let info: PluginInfo = serde_json::from_value(
        shell
            .run(ids::INFO, json!({ "device": device.id }))
            .unwrap(),
    )
    .unwrap();
    assert!(info.active);
    assert_eq!(info.failure, None);
    assert!(info.values.contains(&(ParamId(bl_test_vst3::GAIN), 1.0)));
}

/// Loads the VST3 test plug-in onto `track` and returns its device.
fn load_vst3(shell: &mut Shell, path: &std::path::Path, track: bl_model::TrackId) -> DeviceId {
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(ids::LOAD, json!({ "path": path, "track": track }))
            .unwrap(),
    )
    .unwrap();
    loaded.device
}

// Spec: vst3-hosting R1, R5, R8, R9, R11, R16 (as clap-hosting's). A VST3
// plug-in's state round-trips a saved set, exactly, with the file it came
// from, all through registered commands as an agent runs them.
#[test]
fn a_vst3_plugin_keeps_its_state_with_the_set() {
    let scratch = std::env::temp_dir().join(format!("bl-shell-vst3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let track = audio_track(&shell);
    let device = load_vst3(&mut shell, bl_test_vst3::bundle(), track);
    assert_eq!(shell.editor().undo_history().next(), Some("Insert Device"));

    // The engine runs it, and a parameter change reaches the plug-in and
    // its controller, which `plugin.info` reads.
    let info = wait(&mut shell, device, |i| i.active);
    assert_eq!(info.latency, 0);
    assert_eq!(info.failure, None);
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": bl_test_vst3::GAIN, "value": 0.5 }),
        )
        .unwrap();
    wait(&mut shell, device, |i| {
        value(i, bl_test_vst3::GAIN) == Some(0.5)
    });

    // The controller makes a gesture on Last Note, as its editor would.
    shell
        .vst3()
        .load_state(
            device,
            &bl_plugin_host::vst3::plugin_state(&[], &bl_test_vst3::gesture_state(48, 64)),
        )
        .unwrap();
    wait(&mut shell, device, |i| {
        value(i, bl_test_vst3::LAST_NOTE) == Some(64.0)
    });

    // Saved, the set holds the component's and the controller's state
    // together, taken in without an undo step.
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
    assert_eq!(&state.data[..4], b"BLTV");
    assert_eq!(shell.editor().undo_history().len(), history);

    // Opened again, a new plug-in instance starts from it.
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    let info = wait(&mut shell, device, |i| i.active);
    assert_eq!(value(&info, bl_test_vst3::GAIN), Some(0.5));
    assert_eq!(value(&info, bl_test_vst3::LAST_NOTE), Some(64.0));
    assert_eq!(
        shell.editor().document().device(device).unwrap().plugin,
        Some(state)
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: vst3-hosting R6 (as clap-hosting's R6). A gesture the plug-in's
// controller makes is one undo step, named for its parameter, carrying its
// final value.
#[test]
fn a_change_a_vst3_plugin_makes_itself_is_one_undo_step() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load_vst3(&mut shell, bl_test_vst3::bundle(), track);
    wait(&mut shell, device, |i| i.active);

    let steps = shell.editor().undo_history().len();
    shell
        .vst3()
        .load_state(
            device,
            &bl_plugin_host::vst3::plugin_state(&[], &bl_test_vst3::gesture_state(48, 64)),
        )
        .unwrap();
    assert!(shell.poll());
    assert_eq!(
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&ParamId(bl_test_vst3::LAST_NOTE)),
        Some(&64.0)
    );
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Last Note\"")
    );
}

// Spec: vst3-hosting R10 (as clap-hosting's R10); delay-compensation R6.
// A latency change restarts the plug-in through the shell's poll, and the
// engine recompiles with it, so its delay compensation follows.
#[test]
fn a_vst3_latency_change_restarts_the_plugin_and_reaches_the_engine() {
    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load_vst3(&mut shell, bl_test_vst3::bundle(), track);
    let info = wait(&mut shell, device, |i| i.active);
    assert_eq!(info.latency, 0);
    let before = shell
        .audio()
        .compile_counts()
        .expect("a running engine")
        .programs;

    // A person moves the plug-in's latency up: it takes the value in its
    // next block, its controller asks for a restart, and the host
    // deactivates and activates it again.
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": bl_test_vst3::LATENCY, "value": 256 }),
        )
        .unwrap();
    let info = wait(&mut shell, device, |i| i.latency == 256);
    assert!(info.active);
    assert_eq!(info.failure, None);

    // The shell told the engine, which compiled a program with it.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let programs = shell
            .audio()
            .compile_counts()
            .expect("a running engine")
            .programs;
        if programs > before {
            break;
        }
        assert!(Instant::now() < until, "the engine never recompiled");
        shell.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Spec: vst3-hosting R12 (as clap-hosting's R12). A missing plug-in keeps
// its saved state through a save: nothing can be saved over it.
#[test]
fn a_missing_vst3_plugin_keeps_its_saved_state_through_a_save() {
    let scratch =
        std::env::temp_dir().join(format!("bl-shell-vst3-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    shell.start_audio();
    let track = audio_track(&shell);
    let device = load_vst3(&mut shell, bl_test_vst3::bundle(), track);
    wait(&mut shell, device, |i| i.active);
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": bl_test_vst3::GAIN, "value": 0.5 }),
        )
        .unwrap();
    wait(&mut shell, device, |i| {
        value(i, bl_test_vst3::GAIN) == Some(0.5)
    });
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": scratch.join("Gone.blset") }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    let state = shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.plugin.clone())
        .expect("the state, taken in before the save");

    // The set travels to a machine where the plug-in's file isn't: its path
    // points where none is, and its state stays in the file.
    let bytes = std::fs::read(&path).unwrap();
    let (mut project, info) =
        bl_project::format::decode(&bytes, &bl_project::LoadOptions::default()).unwrap();
    let mut editor = project.editor();
    let moved = bl_model::PluginState {
        path: "/nowhere/Gone.vst3".to_owned(),
        data: state.data.clone(),
    };
    editor
        .amend_plugin_state(device, Some(moved.clone()))
        .unwrap();
    project.document = editor.document().clone();
    project.ids = editor.ids();
    let rewritten = bl_project::format::encode(&project, info.revision + 1).unwrap();
    std::fs::write(&path, rewritten).unwrap();

    // Opened here, the plug-in isn't found; the device passes its input
    // through and says why.
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    let info = wait(&mut shell, device, |i| i.failure.is_some());
    assert!(!info.active, "{info:?}");
    assert!(
        info.failure
            .as_deref()
            .unwrap_or_default()
            .contains("couldn't load"),
        "{info:?}"
    );

    // Saving keeps the state the set holds: nothing can be saved over it.
    assert_eq!(
        shell.editor().document().device(device).unwrap().plugin,
        Some(moved.clone())
    );
    shell.run(commands::SET_SAVE, json!({})).unwrap();
    assert_eq!(
        shell.editor().document().device(device).unwrap().plugin,
        Some(moved)
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: modulation R10
#[test]
fn modulation_never_writes_a_plugins_value_into_the_set() {
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let track = shell.new_id::<bl_model::TrackId>();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": TrackKind::Midi, "name": "Mod", "index": 0 }),
        )
        .unwrap();
    let device = load(&mut shell, track);
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": GAIN, "value": 0.5 }),
        )
        .unwrap();
    wait(&mut shell, device, |i| {
        i.active && value(i, GAIN) == Some(0.5)
    });
    // A plug-in that reports back every value it is given, as some do.
    shell
        .plugins()
        .load_state(device, bl_test_clap::ECHO_PARAMS)
        .unwrap();

    // A one-bar clip whose envelope holds a quarter of the gain's travel:
    // the plug-in plays 1.0 while the set's gain is 0.5.
    let clip = shell.new_id::<bl_model::ClipId>();
    shell
        .run(
            "clip.create",
            json!({ "track": track, "clip": { "id": clip, "name": "", "start": 0,
                    "length": 4 * 705_600, "mute": false,
                    "data": { "midi": { "notes": [] } } } }),
        )
        .unwrap();
    let target = bl_model::AutomationTarget::Device {
        device,
        param: ParamId(GAIN),
    };
    shell
        .run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": clip, "target": target, "at": 0, "value": 0.25 }),
        )
        .unwrap();
    let history = shell.editor().undo_history().len();
    let saved = |shell: &Shell| {
        shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&ParamId(GAIN))
            .copied()
    };

    shell.run("transport.play", json!({})).unwrap();
    wait(&mut shell, device, |i| value(i, GAIN) == Some(1.0));
    // The plug-in goes on reporting what it was given; frames of the app
    // later, none of it is in the set.
    for _ in 0..40 {
        shell.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(saved(&shell), Some(0.5), "the set saves the base");
    assert_eq!(shell.editor().undo_history().len(), history, "no undo step");

    // Stopped, the plug-in has the base again, and the set is as it was.
    shell.run("transport.stop", json!({})).unwrap();
    wait(&mut shell, device, |i| value(i, GAIN) == Some(0.5));
    for _ in 0..20 {
        shell.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(saved(&shell), Some(0.5));
    assert_eq!(shell.editor().undo_history().len(), history);
}

// Spec: clap-hosting R5, R16; WP-5.40 (IMP-03, manual 24.1 and 24.1.2).
// Configure Mode: the switch, the configured list that the panel and the
// choosers show, adding a parameter by moving it in the editor, removing and
// reordering in undo steps, the warning before removing an automated one,
// and the list saved with the set.
#[test]
fn configure_mode_edits_the_plugins_panel_and_is_remembered_with_the_set() {
    let scratch = std::env::temp_dir().join(format!("bl-shell-configure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();

    // Manual 24.1: a plug-in with no more than 64 modifiable parameters
    // starts with them all configured.
    assert_eq!(configured(&mut shell, device).len(), 8);
    assert!(
        parameters(&mut shell, device)
            .params
            .iter()
            .all(|p| p.configured)
    );

    // The Configure switch is view state: it isn't an undo step.
    let steps = shell.editor().undo_history().len();
    let view: DeviceViewState = serde_json::from_value(
        shell
            .run(ids::CONFIGURE, json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert!(
        view.devices
            .iter()
            .find(|d| d.id == device)
            .unwrap()
            .configure
    );
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");

    // Removing a parameter is one undo step and leaves the panel.
    shell
        .run(
            ids::UNCONFIGURE_PARAM,
            json!({ "device": device, "param": LAST_NOTE }),
        )
        .unwrap();
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Unconfigure Plug-In Parameter")
    );
    assert!(!configured(&mut shell, device).contains(&LAST_NOTE));

    // In Configure Mode, a parameter the editor moves joins the panel in
    // the same undo step as the change (manual 24.1.2).
    let steps = shell.editor().undo_history().len();
    plug_in_moves_last_note(&mut shell, device);
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Change \"Last Note\"")
    );
    assert!(configured(&mut shell, device).contains(&LAST_NOTE));

    // Reordering the list is one undo step and moves the parameter to the
    // front of the panel.
    let list = panel_order(&mut shell, device);
    let count = list.len();
    assert!(count > 1);
    shell
        .run(
            ids::MOVE_CONFIGURED_PARAM,
            json!({ "device": device, "from": count - 1, "to": 0 }),
        )
        .unwrap();
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Move Plug-In Parameter")
    );
    assert_eq!(panel_order(&mut shell, device)[0], list[count - 1]);

    // Removing a parameter that has automation warns first (manual 24.1.2),
    // and takes it away when the warning is accepted with `force`.
    shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": track, "at": 0, "value": 0.5,
                    "target": { "type": "device", "device": device, "param": LAST_NOTE } }),
        )
        .unwrap();
    let error = shell
        .run(
            ids::UNCONFIGURE_PARAM,
            json!({ "device": device, "param": LAST_NOTE }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("automation"), "{error}");
    assert!(configured(&mut shell, device).contains(&LAST_NOTE), "kept");
    shell
        .run(
            ids::UNCONFIGURE_PARAM,
            json!({ "device": device, "param": LAST_NOTE, "force": true }),
        )
        .unwrap();
    assert!(!configured(&mut shell, device).contains(&LAST_NOTE));

    // Adding one back by command is one undo step too.
    let steps = shell.editor().undo_history().len();
    shell
        .run(
            ids::CONFIGURE_PARAM,
            json!({ "device": device, "param": LAST_NOTE, "index": 0 }),
        )
        .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Configure Plug-In")
    );
    assert_eq!(panel_order(&mut shell, device)[0], LAST_NOTE);

    // The list is saved with the set and comes back (manual 24.1.2).
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": scratch.join("Configure.blset") }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    let expected = configured(&mut shell, device);
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    shell.poll();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    assert_eq!(configured(&mut shell, device), expected);
    assert_eq!(panel_order(&mut shell, device)[0], LAST_NOTE);

    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: clap-hosting R5; WP-5.40 (manual 24.1.2). Outside Configure Mode a
// parameter the editor moves appears in the automation chooser for now, and
// choosing it there makes it part of the panel.
#[test]
fn a_parameter_moved_outside_configure_mode_joins_the_panel_when_used() {
    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let device = load(&mut shell, track);
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();

    // Take Last Note out of the panel, and move it in the editor with
    // Configure off.
    shell
        .run(
            ids::UNCONFIGURE_PARAM,
            json!({ "device": device, "param": LAST_NOTE }),
        )
        .unwrap();
    plug_in_moves_last_note(&mut shell, device);
    assert!(!configured(&mut shell, device).contains(&LAST_NOTE));

    // Choosing it in the automation choosers makes it permanent.
    shell
        .run(
            "arrangement.show_automation",
            json!({ "track": track,
                    "target": { "type": "device", "device": device, "param": LAST_NOTE } }),
        )
        .unwrap();
    assert!(
        configured(&mut shell, device).contains(&LAST_NOTE),
        "used once, so permanent"
    );
}

/// A MIDI track named `name` with the native instrument Cascade on it, and
/// the test plug-in loaded after it as an audio effect. Returns the track
/// and the plug-in's device.
fn midi_track_with_a_plugin(shell: &mut Shell, name: &str) -> (bl_model::TrackId, DeviceId) {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.name() == name)
        .map(bl_model::Track::id)
        .expect("the new MIDI track");
    shell
        .run(
            "device.add",
            json!({ "track": track,
                    "device": { "kind": { "native": { "device_type": "beatlayer.cascade" } },
                                "name": "Cascade" } }),
        )
        .unwrap();
    let device = load(shell, track);
    (track, device)
}

/// Where `device` sits in `track`'s chain, and what role it has.
fn placed(
    shell: &Shell,
    track: bl_model::TrackId,
    device: DeviceId,
) -> (usize, bl_model::DeviceRole) {
    let document = shell.editor().document();
    let index = document
        .track(track)
        .expect("the track")
        .devices()
        .position(|d| d.id == device)
        .expect("the device is in the chain");
    let role = document.device(device).expect("the device").role;
    (index, role)
}

// Spec: device-chains R2, R3, R14; clap-hosting R1; IMP-24
#[test]
fn a_plugin_can_be_placed_as_a_midi_effect_and_the_placement_saves_and_undoes() {
    let scratch = std::env::temp_dir().join(format!("bl-shell-midi-effect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    shell.start_audio();
    let (track, device) = midi_track_with_a_plugin(&mut shell, "Lead");
    // It loaded after the instrument, as its type goes.
    assert_eq!(
        placed(&shell, track, device),
        (1, bl_model::DeviceRole::AudioEffect)
    );

    let steps = shell.editor().undo_history().len();
    shell
        .run(
            "device.set_role",
            json!({ "device": device, "role": "midi_effect" }),
        )
        .unwrap();
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(
        placed(&shell, track, device),
        (0, bl_model::DeviceRole::MidiEffect),
        "before the instrument, as a MIDI effect"
    );

    // The menu item names no role: the other position is taken.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        placed(&shell, track, device),
        (1, bl_model::DeviceRole::AudioEffect)
    );
    shell
        .invoke("device.set_role", json!({ "device": device }))
        .unwrap();
    assert_eq!(
        placed(&shell, track, device),
        (0, bl_model::DeviceRole::MidiEffect),
        "the menu item takes the other position"
    );

    // Undo and redo are one step each.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        placed(&shell, track, device),
        (1, bl_model::DeviceRole::AudioEffect)
    );
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(
        placed(&shell, track, device),
        (0, bl_model::DeviceRole::MidiEffect)
    );

    // Saved and opened again, it is still a MIDI effect before the
    // instrument.
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": scratch.join("Midi.blset") }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    assert_eq!(
        placed(&shell, track, device),
        (0, bl_model::DeviceRole::MidiEffect)
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: device-chains R2, R3; clap-hosting R1; IMP-24
#[test]
fn a_plugin_without_a_note_output_cannot_be_a_midi_effect() {
    // SAFETY: the plug-in reads this when it is asked for its note ports,
    // and this test runs in a process of its own.
    unsafe { std::env::set_var(bl_test_clap::NOTE_PORTS_VAR, "none") };
    let mut shell = Shell::headless();
    shell.start_audio();
    let (track, device) = midi_track_with_a_plugin(&mut shell, "Lead");
    let error = shell
        .run(
            "device.set_role",
            json!({ "device": device, "role": "midi_effect" }),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("note output"),
        "refused: {error}"
    );
    assert_eq!(
        placed(&shell, track, device),
        (1, bl_model::DeviceRole::AudioEffect),
        "it stays where it was"
    );
}

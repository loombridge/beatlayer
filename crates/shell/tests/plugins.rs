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
use bl_test_clap::{GAIN, PLUGIN_ID, PLUGIN_NAME};
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
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 3);
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
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 1);
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

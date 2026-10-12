//! The Device View's commands (WP-2.4; the device chains spec): adding
//! devices as a person does, selecting, folding, the clipboard, renaming,
//! presets, and hot-swapping, each edit one undo step named as Live 12.4.6
//! names it. Devices are the engine's test devices: Beatlayer ships none
//! of its own until WP-2.5.

use std::path::PathBuf;

use bl_engine::testing;
use bl_model::{DeviceId, DeviceRole, ParamId, TrackId, TrackKind};
use bl_shell::device_commands::{DeviceAdded, DeviceViewState, PresetResult};
use bl_shell::{Area, Effect, Shell};
use serde_json::{Value, json};

/// A scratch folder for a User Library, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-devices-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

struct Set {
    shell: Shell,
    midi: TrackId,
    audio: TrackId,
}

/// The default set with its first audio track selected and the Device View
/// focused, as a click in it leaves it.
fn set() -> Set {
    let mut shell = Shell::headless();
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    let (midi, audio) = (top[0], top[2]);
    select_track(&mut shell, audio);
    shell.set_focused_area(Area::Detail);
    Set { shell, midi, audio }
}

fn select_track(shell: &mut Shell, track: TrackId) {
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
}

fn gain() -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn through() -> Value {
    let t = testing::through_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn synth() -> Value {
    json!({
        "kind": { "native": { "device_type": "test.synth" } },
        "name": "Test Synth",
        "role": "instrument",
    })
}

fn add(shell: &mut Shell, device: Value) -> DeviceAdded {
    serde_json::from_value(
        shell
            .invoke("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

fn state(shell: &Shell) -> DeviceViewState {
    shell.device_view_state()
}

fn titles(shell: &Shell) -> Vec<String> {
    state(shell).devices.into_iter().map(|d| d.title).collect()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: device-chains R12, R14
#[test]
fn a_device_added_goes_after_the_selected_one_and_is_selected() {
    let Set {
        mut shell, audio, ..
    } = set();
    assert!(state(&shell).devices.is_empty());
    let first = add(&mut shell, gain());
    assert_eq!((first.track, first.new_track), (audio, false));
    assert_eq!(state(&shell).selected, Some(first.device));
    assert_eq!(last_step(&shell), "Insert Device");
    let second = add(&mut shell, through());
    shell
        .invoke("device.select", json!({ "device": first.device }))
        .unwrap();
    let third = add(&mut shell, gain());
    let ids: Vec<DeviceId> = state(&shell).devices.iter().map(|d| d.id).collect();
    assert_eq!(ids, [first.device, third.device, second.device]);
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(state(&shell).devices.len(), 2);
}

// Spec: device-chains R12 (observed: Insert Device and Track)
#[test]
fn an_instrument_for_an_audio_track_gets_a_new_midi_track() {
    let Set {
        mut shell, midi, ..
    } = set();
    let tracks = shell.editor().document().top_level().len();
    let added = add(&mut shell, synth());
    assert!(added.new_track);
    let doc = shell.editor().document();
    let track = doc.track(added.track).unwrap();
    assert_eq!(track.kind(), TrackKind::Midi);
    assert_eq!(track.name(), format!("{} Test Synth", tracks + 1));
    assert_eq!(doc.top_level().last(), Some(added.track), "at the end");
    assert_eq!(shell.arrangement().selected_tracks(), [added.track]);
    assert_eq!(state(&shell).track, Some(added.track));
    assert_eq!(last_step(&shell), "Insert Device and Track");
    shell.invoke("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().track(added.track).is_none());

    // On a MIDI track, an instrument goes after its MIDI effects and before
    // its audio effects, whatever is selected.
    select_track(&mut shell, midi);
    let fx = add(&mut shell, gain());
    shell
        .invoke("device.select", json!({ "device": fx.device }))
        .unwrap();
    let instrument = add(&mut shell, synth());
    assert!(!instrument.new_track);
    let roles: Vec<DeviceRole> = state(&shell).devices.iter().map(|d| d.role).collect();
    assert_eq!(roles, [DeviceRole::Instrument, DeviceRole::AudioEffect]);
    // A second instrument is refused until racks arrive.
    assert!(
        shell
            .invoke("device.add", json!({ "device": synth() }))
            .is_err()
    );
}

// Spec: device-chains R11 (fold isn't an undo step)
#[test]
fn folding_changes_only_the_view() {
    let Set { mut shell, .. } = set();
    let added = add(&mut shell, gain());
    let steps = shell.editor().undo_history().len();
    assert!(shell.can_invoke("device.fold", &json!({})));
    shell.invoke("device.fold", json!({})).unwrap();
    assert!(state(&shell).devices[0].folded);
    shell
        .invoke(
            "device.fold",
            json!({ "device": added.device, "folded": false }),
        )
        .unwrap();
    assert!(!state(&shell).devices[0].folded);
    assert_eq!(shell.editor().undo_history().len(), steps);
}

// Spec: device-chains R14, R20
#[test]
fn keys_and_menu_items_act_on_the_selected_device() {
    let Set { mut shell, .. } = set();
    let added = add(&mut shell, gain());

    // The activator: Change "Device On".
    shell
        .invoke(
            "device.set_enabled",
            json!({ "device": added.device, "enabled": false }),
        )
        .unwrap();
    assert!(!state(&shell).devices[0].enabled);
    assert_eq!(last_step(&shell), "Change \"Device On\"");

    // Show Preset Name, ticked in the menu while it's on.
    assert_eq!(
        shell.switch_state("device.set_show_preset_name"),
        Some(true)
    );
    shell
        .invoke("device.set_show_preset_name", json!({}))
        .unwrap();
    assert_eq!(
        shell.switch_state("device.set_show_preset_name"),
        Some(false)
    );
    assert_eq!(last_step(&shell), "Toggle Show Preset Name");

    // Rename: the key asks the title bar for a name; the name renames.
    assert!(shell.can_invoke("device.rename", &json!({})));
    shell.invoke("device.rename", json!({})).unwrap();
    assert_eq!(
        shell.take_effects(),
        vec![Effect::RenameDevice(added.device)]
    );
    shell
        .invoke("device.rename", json!({ "name": "Trim" }))
        .unwrap();
    assert_eq!(last_step(&shell), "Change \"Device Name\"");
    // Show Preset Name is off, so the title is still the device's own name.
    assert_eq!(titles(&shell), ["Test Gain"]);
    shell
        .invoke("device.set_show_preset_name", json!({}))
        .unwrap();
    assert_eq!(titles(&shell), ["Trim"]);

    // Delete.
    assert!(shell.can_invoke("device.remove", &json!({})));
    shell.invoke("device.remove", json!({})).unwrap();
    assert!(state(&shell).devices.is_empty());
    assert_eq!(state(&shell).selected, None);
    assert_eq!(last_step(&shell), "Delete Device(s)");
    assert!(!shell.can_invoke("device.remove", &json!({})));
}

// Spec: device-chains R15
#[test]
fn cut_copy_paste_and_duplicate() {
    let Set {
        mut shell, midi, ..
    } = set();
    let a = add(&mut shell, gain());
    let b = add(&mut shell, through());
    assert!(!shell.can_invoke("device.paste", &json!({})));

    // Copy, then paste before the selected device; the copy is selected.
    shell
        .invoke("device.select", json!({ "device": a.device }))
        .unwrap();
    shell.invoke("device.copy", json!({})).unwrap();
    shell
        .invoke("device.select", json!({ "device": b.device }))
        .unwrap();
    let pasted: DeviceAdded =
        serde_json::from_value(shell.invoke("device.paste", json!({})).unwrap()).unwrap();
    assert_eq!(
        titles(&shell),
        ["Test Gain", "Test Gain", "Test Pass-Through"]
    );
    assert_eq!(state(&shell).devices[1].id, pasted.device);
    assert_eq!(state(&shell).selected, Some(pasted.device));
    assert_eq!(last_step(&shell), "Copy Device");

    // Duplicate puts the copy straight after.
    let dup: DeviceAdded =
        serde_json::from_value(shell.invoke("device.duplicate", json!({})).unwrap()).unwrap();
    assert_eq!(state(&shell).devices[2].id, dup.device);
    assert_eq!(last_step(&shell), "Duplicate Device");

    // Cut deletes and keeps it for pasting, on another track too.
    shell
        .invoke("device.select", json!({ "device": b.device }))
        .unwrap();
    shell.invoke("device.cut", json!({})).unwrap();
    assert_eq!(last_step(&shell), "Delete Device(s)");
    assert_eq!(state(&shell).devices.len(), 3);
    select_track(&mut shell, midi);
    shell.invoke("device.paste", json!({})).unwrap();
    assert_eq!(titles(&shell), ["Test Pass-Through"]);
}

// Spec: device-chains R16, R17, R18, R13
#[test]
fn presets_save_load_and_default() {
    let library = Scratch::new("presets");
    let Set { mut shell, .. } = set();
    shell.set_user_library(Some(library.0.clone()));
    let a = add(&mut shell, gain());
    shell
        .invoke(
            "device.set_param",
            json!({ "device": a.device, "param": 0, "value": 0.5 }),
        )
        .unwrap();
    let saved: PresetResult =
        serde_json::from_value(shell.invoke("device.save_preset", json!({})).unwrap()).unwrap();
    let expected = library
        .0
        .join("Presets/Audio Effects/Test Gain/Test Gain.blpreset");
    assert_eq!(PathBuf::from(&saved.path), expected);
    assert!(expected.is_file());

    // Loading it into a fresh device sets its values and its title.
    let b = add(&mut shell, gain());
    shell
        .invoke(
            "device.load_preset",
            json!({ "device": b.device, "path": saved.path }),
        )
        .unwrap();
    let doc = shell.editor().document();
    assert_eq!(
        doc.device(b.device).unwrap().params.get(&ParamId(0)),
        Some(&0.5)
    );
    assert_eq!(last_step(&shell), "Load Preset");
    // A preset for another kind of device is refused.
    let c = add(&mut shell, through());
    assert!(
        shell
            .invoke(
                "device.load_preset",
                json!({ "device": c.device, "path": saved.path }),
            )
            .is_err()
    );

    // Save as Default Preset: new devices of the type start with it.
    shell
        .invoke("device.select", json!({ "device": a.device }))
        .unwrap();
    shell
        .invoke("device.save_default_preset", json!({}))
        .unwrap();
    assert!(
        library
            .0
            .join("Defaults/Audio Effects/Test Gain.blpreset")
            .is_file()
    );
    let d = add(&mut shell, gain());
    assert_eq!(d.default_preset.as_deref(), Some("Test Gain"));
    assert_eq!(
        shell.message().map(|m| m.text.as_str()),
        Some("Using Default Preset Test Gain.blpreset.")
    );
    let doc = shell.editor().document();
    assert_eq!(
        doc.device(d.device).unwrap().params.get(&ParamId(0)),
        Some(&0.5)
    );
    assert_eq!(
        last_step(&shell),
        "Insert Device",
        "one step with its preset"
    );
}

// Spec: device-chains R19
#[test]
fn hot_swapping_loads_the_browsers_preset_into_the_device() {
    let library = Scratch::new("hot-swap");
    let Set { mut shell, .. } = set();
    shell.set_user_library(Some(library.0.clone()));
    let a = add(&mut shell, gain());
    shell
        .invoke(
            "device.set_param",
            json!({ "device": a.device, "param": 0, "value": 0.25 }),
        )
        .unwrap();
    let saved: PresetResult =
        serde_json::from_value(shell.invoke("device.save_preset", json!({})).unwrap()).unwrap();
    let b = add(&mut shell, gain());

    shell
        .invoke("device.hot_swap", json!({ "device": b.device }))
        .unwrap();
    let hot = state(&shell).hot_swap.unwrap();
    assert_eq!(
        (hot.device, hot.label.as_str()),
        (b.device, "Swapping Audio Effect")
    );
    assert!(shell.browser_visible());
    // The browser shows the User Library with the device's folder open.
    let browser = shell.browser_view();
    let folder = library.0.join("Presets/Audio Effects/Test Gain");
    assert!(browser.position_of(&folder).is_some(), "{folder:?}");
    assert!(browser.position_of(&PathBuf::from(&saved.path)).is_some());

    shell
        .invoke("browser.select", json!({ "paths": [saved.path] }))
        .unwrap();
    shell.set_focused_area(Area::Browser);
    assert!(shell.can_invoke("browser.load_selected", &json!({})));
    shell.invoke("browser.load_selected", json!({})).unwrap();
    let doc = shell.editor().document();
    assert_eq!(
        doc.device(b.device).unwrap().params.get(&ParamId(0)),
        Some(&0.25)
    );
    assert_eq!(doc.device(b.device).unwrap().title(), "Test Gain");

    shell
        .invoke("device.hot_swap", json!({ "on": false }))
        .unwrap();
    assert!(state(&shell).hot_swap.is_none());
}

// Spec: device-chains R20
#[test]
fn agents_reach_every_device_command() {
    let Set {
        mut shell, audio, ..
    } = set();
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": gain(), "track": audio, "index": 0 }),
            )
            .unwrap(),
    )
    .unwrap();
    for (id, args) in [
        ("device_view.state", json!({})),
        ("device.select", json!({ "device": added.device })),
        (
            "device.fold",
            json!({ "device": added.device, "folded": true }),
        ),
        ("device.copy", json!({ "device": added.device })),
        ("device.paste", json!({ "track": audio })),
        ("device.duplicate", json!({ "device": added.device })),
        (
            "device.hot_swap",
            json!({ "device": added.device, "on": true }),
        ),
        ("device.hot_swap", json!({ "on": false })),
        ("device.cut", json!({ "device": added.device })),
    ] {
        shell
            .run(id, args.clone())
            .unwrap_or_else(|e| panic!("{id} {args}: {e}"));
    }
    assert_eq!(state(&shell).devices.len(), 2);
    for id in bl_shell::device_commands::IDS {
        let spec = shell.registry().get(id).unwrap();
        assert!(
            spec.permission.is_some() && !spec.description.is_empty(),
            "{id}"
        );
    }
}

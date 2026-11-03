//! Modular through the shell (WP-9.36; the modular device spec, R1, R6, R8,
//! R9): the three devices, the `modular.` commands with their readback, a
//! set and a preset that hold the patch, and patch parameters that macros
//! and automation reach like any parameter.

use std::path::PathBuf;

use bl_model::modular::{self, BlockId};
use bl_model::{DeviceId, DeviceKind, DeviceRole};
use bl_shell::Area;
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::device_commands::{DeviceAdded, PresetResult};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell_spec::modular::PatchState;
use serde_json::{Value, json};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-modular-{}-{name}", std::process::id()));
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

fn add(shell: &mut Shell, device_type: &str) -> DeviceId {
    shell.set_device_types(bl_devices::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let t = bl_devices::device_types()
        .into_iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type: d } if d == device_type))
        .expect("the device type is registered");
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn patch(shell: &mut Shell, device: DeviceId) -> PatchState {
    serde_json::from_value(shell.run("modular.patch", json!({ "device": device })).unwrap())
        .unwrap()
}

fn last_step(shell: &Shell) -> String {
    shell.editor().undo_history().next().unwrap_or_default().to_owned()
}

// Spec: modular-device R1
#[test]
fn each_kind_starts_with_its_in_and_out_blocks_joined() {
    for (device_type, role, blocks, cables) in [
        (modular::INSTRUMENT, DeviceRole::Instrument, 5, 6),
        (modular::EFFECT, DeviceRole::AudioEffect, 2, 2),
        (modular::MIDI_EFFECT, DeviceRole::MidiEffect, 2, 1),
    ] {
        let mut shell = Shell::headless();
        let device = add(&mut shell, device_type);
        let state = patch(&mut shell, device);
        assert_eq!((state.blocks.len(), state.cables.len()), (blocks, cables), "{device_type}");
        assert_eq!(shell.editor().document().device(device).unwrap().role, role);
        assert_eq!(state.unknown_blocks, 0);
    }
    let mut shell = Shell::headless();
    let device = add(&mut shell, modular::INSTRUMENT);
    let state = patch(&mut shell, device);
    assert_eq!(state.voices, 8, "8 voices in a new device");
    let osc = state.blocks.iter().find(|b| b.kind == "oscillator").unwrap();
    assert_eq!(osc.name, "Oscillator");
    assert_eq!(
        osc.inputs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["pitch", "pm", "sync"]
    );
    assert!(osc.settings.iter().any(|s| s.id == "coarse" && s.unit == " st"));
}

// Spec: modular-device R9, R3, R2
#[test]
fn the_commands_edit_the_patch_one_undo_step_each_and_modular_patch_reads_it_back() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, modular::EFFECT);
    let run = |shell: &mut Shell, id: &str, args: Value| shell.run(id, args);
    let steps = shell.editor().undo_history().len();
    run(&mut shell, "modular.add_block", json!({ "device": device, "block_type": "filter", "id": 10, "x": 5.0, "y": 6.0 })).unwrap();
    assert_eq!(last_step(&shell), "Add Block");
    let filter = patch(&mut shell, device).blocks.into_iter().find(|b| b.id == BlockId(10)).unwrap();
    assert_eq!((filter.x, filter.y), (5.0, 6.0));
    // Without an ID the next free one is used.
    run(&mut shell, "modular.add_block", json!({ "device": device, "block_type": "delay", "id": null, "x": 0.0, "y": 0.0 })).unwrap();
    assert!(patch(&mut shell, device).blocks.iter().any(|b| b.id == BlockId(11) && b.kind == "delay"));
    run(&mut shell, "modular.connect", json!({ "device": device, "from": 10, "from_port": "out", "to": 11, "to_port": "in" })).unwrap();
    assert_eq!(last_step(&shell), "Connect Blocks");
    // R3: a loop that runs through a Delay's audio input is allowed.
    run(&mut shell, "modular.connect", json!({ "device": device, "from": 11, "from_port": "out", "to": 10, "to_port": "in" })).unwrap();
    run(&mut shell, "modular.disconnect", json!({ "device": device, "from": 11, "from_port": "out", "to": 10, "to_port": "in" })).unwrap();
    // One with none is refused with the reason, and changes nothing.
    run(&mut shell, "modular.add_block", json!({ "device": device, "block_type": "amplifier", "id": 12, "x": 0.0, "y": 0.0 })).unwrap();
    run(&mut shell, "modular.connect", json!({ "device": device, "from": 10, "from_port": "out", "to": 12, "to_port": "in" })).unwrap();
    let before = shell.editor().document().clone();
    let error = run(&mut shell, "modular.connect", json!({ "device": device, "from": 12, "from_port": "out", "to": 10, "to_port": "in" })).unwrap_err();
    assert!(error.to_string().contains("cycle"), "{error}");
    assert_eq!(shell.editor().document(), &before);
    // R2: a second cable into an input, a port the block lacks and a block
    // the patch lacks are refused too.
    for args in [
        json!({ "device": device, "from": 11, "from_port": "out", "to": 12, "to_port": "in" }),
        json!({ "device": device, "from": 10, "from_port": "nope", "to": 12, "to_port": "cv" }),
        json!({ "device": device, "from": 10, "from_port": "out", "to": 99, "to_port": "in" }),
    ] {
        assert!(run(&mut shell, "modular.connect", args).is_err());
    }
    assert_eq!(shell.editor().document(), &before);
    run(&mut shell, "modular.set", json!({ "device": device, "block": 10, "setting": "cutoff", "value": 440.0 })).unwrap();
    assert_eq!(last_step(&shell), "Change Block Setting");
    let state = patch(&mut shell, device);
    let cutoff = state.blocks.iter().find(|b| b.id == BlockId(10)).unwrap().settings.iter().find(|s| s.id == "cutoff").unwrap().clone();
    assert_eq!((cutoff.value, cutoff.default), (440.0, 1000.0));
    assert!(run(&mut shell, "modular.set", json!({ "device": device, "block": 10, "setting": "cutoff", "value": 5.0 })).is_err(), "below the range");
    run(&mut shell, "modular.move_block", json!({ "device": device, "block": 10, "x": 1.0, "y": 2.0 })).unwrap();
    assert_eq!(last_step(&shell), "Move Block");
    run(&mut shell, "modular.remove_block", json!({ "device": device, "block": 10 })).unwrap();
    let state = patch(&mut shell, device);
    assert!(state.blocks.iter().all(|b| b.id != BlockId(10)));
    assert!(state.cables.iter().all(|c| c.from != BlockId(10) && c.to != BlockId(10)), "its cables went with it");
    // Every edit is one undo step: undoing them all returns the new patch.
    while shell.editor().undo_history().len() > steps {
        shell.run("edit.undo", json!({})).unwrap();
    }
    assert_eq!(patch(&mut shell, device).blocks.len(), 2);
}

// Spec: modular-device R6, R16
#[test]
fn a_patch_parameter_is_a_device_parameter_that_automation_and_macros_reach() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, modular::INSTRUMENT);
    let osc = patch(&mut shell, device).blocks.iter().find(|b| b.kind == "oscillator").unwrap().id;
    shell.run("modular.expose", json!({ "device": device, "block": osc, "setting": "coarse", "exposed": true })).unwrap();
    assert_eq!(last_step(&shell), "Change Patch Parameter");
    let state = patch(&mut shell, device);
    let param = &state.params[0];
    assert_eq!((param.slot, param.name.as_str()), (1, "Oscillator Coarse"));
    // It is listed among the device's parameters, automatable, and holds the
    // setting's place in its range.
    let listed: Value = shell.run("device.parameters", json!({ "device": device })).unwrap();
    let listed = listed["params"].as_array().unwrap().iter().find(|p| p["id"] == json!(param.param)).cloned().unwrap();
    assert_eq!(listed["automatable"], json!(true));
    assert!((listed["value"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    // Setting the setting moves the parameter in the same step.
    shell.run("modular.set", json!({ "device": device, "block": osc, "setting": "coarse", "value": 12.0 })).unwrap();
    let held = shell.editor().document().device(device).unwrap().params[&param.param];
    assert!((held - (0.5 + 12.0 / 96.0)).abs() < 1e-9);
    // Stopping removes the parameter and its value.
    shell.run("modular.expose", json!({ "device": device, "block": osc, "setting": "coarse", "exposed": false })).unwrap();
    assert!(patch(&mut shell, device).params.is_empty());
    assert!(!shell.editor().document().device(device).unwrap().params.contains_key(&param.param));
}

// Spec: modular-device R6
#[test]
fn a_macro_maps_to_a_patch_parameter_like_any_parameter() {
    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell.invoke("arrangement.select_tracks", json!({ "tracks": [audio] })).unwrap();
    shell.set_focused_area(Area::Detail);
    let rack: DeviceAdded = serde_json::from_value(shell.invoke("device.add", json!({ "device": { "kind": { "rack": { "kind": "audio_effect" } }, "name": "Rack", "role": "audio_effect" } })).unwrap()).unwrap();
    let chain = shell.new_id::<bl_model::ChainId>();
    shell.invoke("chain.add", json!({ "rack": rack.device, "index": 0, "chain": { "id": chain, "name": "Range", "devices": [] } })).unwrap();
    let t = bl_devices::device_types().into_iter().find(|t| matches!(&t.kind, DeviceKind::Native { device_type } if device_type == modular::EFFECT)).unwrap();
    let modular: DeviceAdded = serde_json::from_value(shell.invoke("device.add", json!({ "chain": chain, "device": { "kind": t.kind, "name": t.name, "role": t.role } })).unwrap()).unwrap();
    let device = modular.device;
    shell.run("modular.add_block", json!({ "device": device, "block_type": "filter", "id": 10, "x": 0.0, "y": 0.0 })).unwrap();
    shell.run("modular.expose", json!({ "device": device, "block": 10, "setting": "cutoff", "exposed": true })).unwrap();
    let param = patch(&mut shell, device).params[0].param;
    shell
        .invoke(
            "rack.map_macro",
            json!({ "rack": rack.device, "macro_number": 1, "target": { "type": "device", "device": device, "param": param.0 } }),
        )
        .unwrap();
    let macro_param = bl_model::macro_param(1);
    shell.invoke("device.set_param", json!({ "device": rack.device, "param": macro_param.0, "value": 1.0 })).unwrap();
    let moved = shell.editor().document().device(device).unwrap().params[&param];
    assert!((moved - 1.0).abs() < 1e-9, "the macro at its top takes the cutoff to the top of its range: {moved}");
}

// Spec: modular-device R8
#[test]
fn a_set_saves_and_reopens_with_its_patch_and_a_preset_holds_it() {
    let scratch = Scratch::new("save");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.set_user_library(Some(scratch.0.join("library")));
    let device = add(&mut shell, modular::INSTRUMENT);
    shell.run("modular.add_block", json!({ "device": device, "block_type": "lfo", "id": 40, "x": 9.0, "y": 8.0 })).unwrap();
    shell.run("modular.set", json!({ "device": device, "block": 40, "setting": "rate", "value": 7.5 })).unwrap();
    shell.run("modular.expose", json!({ "device": device, "block": 40, "setting": "rate" , "exposed": true})).unwrap();
    let saved_patch = patch(&mut shell, device);
    // The preset holds the whole patch and its layout.
    let preset: PresetResult = serde_json::from_value(shell.invoke("device.save_preset", json!({ "device": device })).unwrap()).unwrap();
    let text = std::fs::read_to_string(&preset.path).unwrap();
    assert!(text.contains("\"lfo\"") && text.contains("7.5"));
    let other = add(&mut shell, modular::INSTRUMENT);
    assert_ne!(patch(&mut shell, other).blocks.len(), saved_patch.blocks.len());
    shell.invoke("device.load_preset", json!({ "device": other, "path": preset.path })).unwrap();
    assert_eq!(last_step(&shell), "Load Preset");
    let loaded = patch(&mut shell, other);
    assert_eq!(loaded.blocks, saved_patch.blocks);
    assert_eq!(loaded.cables, saved_patch.cables);
    assert_eq!(loaded.params.len(), 1);
    // A set saves it with the device and opens it again.
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell.run("set.save_as", json!({ "path": scratch.0.join("Modular.blset").to_string_lossy() })).unwrap(),
    )
    .unwrap();
    let path = saved.path.expect("the set was saved");
    shell.run("modular.remove_block", json!({ "device": device, "block": 40 })).unwrap();
    shell.run("set.discard_changes", json!({})).unwrap();
    shell.run("set.open", json!({ "path": path })).unwrap();
    assert_eq!(patch(&mut shell, device), saved_patch);
}

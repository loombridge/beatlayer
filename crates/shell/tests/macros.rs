//! Macros through the shell's commands (WP-5.4; the macros spec, R5 to
//! R19): mapping, the law's values, the hold a mapping gives, the undo
//! steps and the readback.

use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

const UTILITY: &str = "beatlayer.utility";
const WIDTH: u32 = 6;
const BALANCE: u32 = 7;

struct Set {
    shell: Shell,
    rack: DeviceId,
    utility: DeviceId,
}

fn set() -> Set {
    let mut shell = Shell::headless();
    let audio: TrackId = shell
        .editor()
        .document()
        .top_level()
        .nth(2)
        .expect("an audio track");
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let rack: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({
                    "device": {
                        "kind": { "rack": { "kind": "audio_effect" } },
                        "name": "Rack",
                        "role": "audio_effect",
                    },
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let range = shell.new_id::<bl_model::ChainId>();
    shell
        .invoke(
            "chain.add",
            json!({ "rack": rack.device, "index": 0, "chain": { "id": range, "name": "Range", "devices": [] } }),
        )
        .unwrap();
    let utility: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({
                    "chain": range,
                    "device": {
                        "kind": { "native": { "device_type": UTILITY } },
                        "name": "Utility",
                        "role": "audio_effect",
                    },
                }),
            )
            .unwrap(),
    )
    .unwrap();
    Set {
        shell,
        rack: rack.device,
        utility: utility.device,
    }
}

fn param(set: &Set, device: DeviceId, param: u32) -> Option<f64> {
    set.shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.params.get(&bl_model::ParamId(param)).copied())
}

fn macro_value(set: &Set, number: u8) -> f64 {
    param(set, set.rack, bl_model::macro_param(number).0).unwrap_or(0.0)
}

fn map(set: &mut Set, number: u8, param: u32) {
    set.shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": set.rack,
                "macro_number": number,
                "target": { "type": "device", "device": set.utility, "param": param },
            }),
        )
        .unwrap();
}

fn last_undo(set: &Set) -> String {
    set.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: macros R1, R12
#[test]
fn set_param_moves_a_macro_like_any_parameter() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    // R6: the macro first moves to the position that gives Width its value,
    // 100 of 0 to 200.
    assert!((macro_value(&set, 1) - 0.5).abs() < 1e-9);
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.25 }),
        )
        .unwrap();
    assert!((param(&set, set.utility, WIDTH).unwrap() - 50.0).abs() < 1e-9);
    assert_eq!(last_undo(&set), "Change \"Width\"");
}

// Spec: macros R13
#[test]
fn moving_a_macro_is_one_undo_step_named_for_it() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    let before = set.shell.editor().undo_history().count();
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.75 }),
        )
        .unwrap();
    assert_eq!(set.shell.editor().undo_history().count(), before + 1);
    assert_eq!(last_undo(&set), "Change \"Width\"");
}

// Spec: macros R11
#[test]
fn a_mapped_parameter_refuses_set_param_and_names_its_macro() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    let error = set
        .shell
        .invoke(
            "device.set_param",
            json!({ "device": set.utility, "param": WIDTH, "value": 10.0 }),
        )
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("Width"), "{text}");
}

// Spec: macros R11
#[test]
fn unmapping_hands_the_parameter_back_where_it_is() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.25 }),
        )
        .unwrap();
    set.shell
        .invoke(
            "rack.unmap_macro",
            json!({
                "rack": set.rack,
                "target": { "type": "device", "device": set.utility, "param": WIDTH },
            }),
        )
        .unwrap();
    assert!((param(&set, set.utility, WIDTH).unwrap() - 50.0).abs() < 1e-9);
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.utility, "param": WIDTH, "value": 20.0 }),
        )
        .unwrap();
    assert!((param(&set, set.utility, WIDTH).unwrap() - 20.0).abs() < 1e-9);
}

// Spec: macros R9
#[test]
fn each_mapping_edit_is_one_undo_step() {
    let mut set = set();
    let before = set.shell.editor().undo_history().count();
    map(&mut set, 1, WIDTH);
    assert_eq!(set.shell.editor().undo_history().count(), before + 1);
    let target = json!({ "type": "device", "device": set.utility, "param": WIDTH });
    set.shell
        .invoke(
            "rack.set_mapping_range",
            json!({ "rack": set.rack, "target": target, "min": 0.25, "max": 0.75 }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Change Macro Mapping");
    set.shell
        .invoke(
            "rack.invert_mapping",
            json!({ "rack": set.rack, "target": target }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Invert Macro Mapping");
    set.shell
        .invoke(
            "rack.unmap_macro",
            json!({ "rack": set.rack, "target": target }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Unmap Macro");
}

// Spec: macros R15
#[test]
fn randomize_moves_only_mapped_macros_not_excluded() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    map(&mut set, 2, BALANCE);
    set.shell
        .invoke(
            "rack.set_macro_randomize_excluded",
            json!({ "rack": set.rack, "macro_number": 2, "excluded": true }),
        )
        .unwrap();
    let excluded = macro_value(&set, 2);
    set.shell
        .invoke(
            "rack.randomize_macros",
            json!({ "rack": set.rack, "seed": 7 }),
        )
        .unwrap();
    let first = macro_value(&set, 1);
    assert_ne!(first, 0.5, "macro 1 moved");
    assert!((macro_value(&set, 2) - excluded).abs() < 1e-12, "excluded");
    set.shell
        .invoke(
            "rack.randomize_macros",
            json!({ "rack": set.rack, "seed": 7 }),
        )
        .unwrap();
    assert!(
        (macro_value(&set, 1) - first).abs() < 1e-12,
        "the same seed"
    );
}

// Spec: macros R14
#[test]
fn rename_colour_and_info_text_are_one_undo_step_each_and_save() {
    let mut set = set();
    for (id, args, label) in [
        (
            "rack.rename_macro",
            json!({ "rack": set.rack, "macro_number": 1, "name": "Brightness" }),
            "Rename Macro",
        ),
        (
            "rack.set_macro_colour",
            json!({ "rack": set.rack, "macro_number": 1, "colour": 3 }),
            "Change Macro Color",
        ),
        (
            "rack.set_macro_info_text",
            json!({ "rack": set.rack, "macro_number": 1, "info": "Opens it up" }),
            "Change Macro Info Text",
        ),
    ] {
        set.shell.invoke(id, args).unwrap();
        assert_eq!(last_undo(&set), label);
    }
    let rack = set.shell.editor().document().device(set.rack).unwrap();
    let setting = rack.macros.setting(1);
    assert_eq!(setting.name, "Brightness");
    assert_eq!(setting.colour, Some(3));
    assert_eq!(setting.info, "Opens it up");
}

// Spec: macros R16
#[test]
fn exclusion_switches_in_one_undo_step_and_saves() {
    let mut set = set();
    set.shell
        .invoke(
            "rack.set_macro_randomize_excluded",
            json!({ "rack": set.rack, "macro_number": 9, "excluded": true }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Exclude Macro from Randomization");
    let rack = set.shell.editor().document().device(set.rack).unwrap();
    assert!(rack.macros.setting(9).randomize_excluded);
}

// Spec: macros R19
#[test]
fn rack_macros_reads_back_names_values_mappings_and_the_count() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    let state: Value = set
        .shell
        .invoke("rack.macros", json!({ "rack": set.rack }))
        .unwrap();
    assert_eq!(state["count"], 8);
    let macros = state["macros"].as_array().unwrap();
    assert_eq!(macros.len(), 16);
    assert_eq!(macros[0]["name"], "Width");
    assert_eq!(macros[0]["mappings"].as_array().unwrap().len(), 1);
    assert_eq!(macros[0]["mappings"][0]["param"], "Width");
    assert_eq!(macros[0]["mappings"][0]["min"], 0.0);
    assert_eq!(macros[0]["mappings"][0]["max"], 200.0);
    set.shell
        .invoke(
            "rack.set_macro_count",
            json!({ "rack": set.rack, "count": 12 }),
        )
        .unwrap();
    let state: Value = set
        .shell
        .invoke("rack.macros", json!({ "rack": set.rack }))
        .unwrap();
    assert_eq!(state["count"], 12);
}

// Spec: macros R3
#[test]
fn map_mode_turns_on_and_off_as_view_state() {
    let mut set = set();
    let before = set.shell.editor().undo_history().count();
    set.shell
        .invoke("rack.map_mode", json!({ "rack": set.rack, "on": true }))
        .unwrap();
    assert_eq!(set.shell.devices().map_mode(), Some(set.rack));
    set.shell
        .invoke("rack.map_mode", json!({ "rack": set.rack, "on": false }))
        .unwrap();
    assert_eq!(set.shell.devices().map_mode(), None);
    assert_eq!(set.shell.editor().undo_history().count(), before, "no step");
}

// Spec: macros R4
#[test]
fn a_chain_mixer_maps_and_follows_the_macro() {
    let mut set = set();
    let chain = set
        .shell
        .editor()
        .document()
        .device(set.rack)
        .unwrap()
        .chains[0]
        .id;
    set.shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": set.rack,
                "macro_number": 1,
                "target": { "type": "chain", "chain": chain, "param": "volume" },
            }),
        )
        .unwrap();
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.0 }),
        )
        .unwrap();
    let mixer = set.shell.editor().document().chain(chain).unwrap().mixer;
    assert!(mixer.volume < 1.0, "the macro drives the chain's volume");
}

// Spec: macros R2
#[test]
fn the_macro_count_stays_in_range() {
    let mut set = set();
    assert!(
        set.shell
            .invoke(
                "rack.set_macro_count",
                json!({ "rack": set.rack, "count": 0 })
            )
            .is_err()
    );
    assert!(
        set.shell
            .invoke(
                "rack.set_macro_count",
                json!({ "rack": set.rack, "count": 17 })
            )
            .is_err()
    );
}

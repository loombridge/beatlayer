//! Macros through the shell's commands (WP-5.4; the macros spec, R5 to
//! R19): mapping, the law's values, the hold a mapping gives, the undo
//! steps and the readback.

use bl_audio_io::{BackendKind, NullBackend, NullControl, NullOptions, Pacing};
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, DeviceId, TrackId};
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Area, Effect, Shell, testing};
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

// Spec: macros R14. Rename names a selected macro; choosing another device
// ends that selection, so Rename names the device again.
#[test]
fn choosing_a_device_takes_rename_back_from_a_macro() {
    let mut set = set();
    set.shell
        .invoke(
            "rack.select_macro",
            json!({ "rack": set.rack, "macro_number": 1 }),
        )
        .unwrap();
    set.shell.invoke("device.rename", json!({})).unwrap();
    assert_eq!(
        set.shell.take_effects(),
        vec![Effect::RenameMacro(set.rack, 1)]
    );
    let added: DeviceAdded = serde_json::from_value(
        set.shell
            .invoke(
                "device.add",
                json!({
                    "device": {
                        "kind": { "native": { "device_type": UTILITY } },
                        "name": "Second",
                        "role": "audio_effect",
                    },
                }),
            )
            .unwrap(),
    )
    .unwrap();
    set.shell.invoke("device.rename", json!({})).unwrap();
    assert_eq!(
        set.shell.take_effects(),
        vec![Effect::RenameDevice(added.device)]
    );
}

// Spec: macros R3, R14. A rack that's gone takes its Map mode and its
// selected macro with it.
#[test]
fn deleting_a_rack_forgets_its_macro_and_its_map_mode() {
    let mut set = set();
    set.shell
        .invoke(
            "rack.select_macro",
            json!({ "rack": set.rack, "macro_number": 3 }),
        )
        .unwrap();
    set.shell
        .invoke("rack.map_mode", json!({ "rack": set.rack, "on": true }))
        .unwrap();
    set.shell
        .invoke("device.remove", json!({ "device": set.rack }))
        .unwrap();
    assert_eq!(set.shell.devices().map_mode(), None);
    assert_eq!(set.shell.devices().macro_selected(), None);
}

// Spec: macros R29
#[test]
fn variations_rename_duplicate_and_delete_in_one_undo_step_each() {
    let mut set = set();
    set.shell
        .invoke("rack.store_variation", json!({ "rack": set.rack }))
        .unwrap();
    assert_eq!(last_undo(&set), "Store Macro Variation");
    set.shell
        .invoke(
            "rack.rename_variation",
            json!({ "rack": set.rack, "index": 0, "name": "Quiet" }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Rename Macro Variation");
    set.shell
        .invoke(
            "rack.duplicate_variation",
            json!({ "rack": set.rack, "index": 0 }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Duplicate Macro Variation");
    // Moving a macro and then overwriting records the new values.
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.9 }),
        )
        .unwrap();
    set.shell
        .invoke(
            "rack.overwrite_variation",
            json!({ "rack": set.rack, "index": 0 }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Overwrite Macro Variation");
    set.shell
        .invoke(
            "rack.delete_variation",
            json!({ "rack": set.rack, "index": 1 }),
        )
        .unwrap();
    assert_eq!(last_undo(&set), "Delete Macro Variation");
    let state: Value = set
        .shell
        .invoke("rack.macros", json!({ "rack": set.rack }))
        .unwrap();
    let variations = state["variations"].as_array().unwrap();
    assert_eq!(variations.len(), 1);
    assert_eq!(variations[0]["name"], "Quiet");

    let before = set.shell.editor().undo_history().count();
    set.shell
        .invoke(
            "rack.launch_variation",
            json!({ "rack": set.rack, "index": 0 }),
        )
        .unwrap();
    assert_eq!(set.shell.editor().undo_history().count(), before + 1);
    assert_eq!(last_undo(&set), "Launch Macro Variation");
}

// Spec: macros R30
#[test]
fn an_excluded_macro_keeps_its_value_when_a_variation_launches() {
    let mut set = set();
    map(&mut set, 1, WIDTH);
    // Macro 1 stands at Width's value, 0.5; the variation stores it.
    set.shell
        .invoke("rack.store_variation", json!({ "rack": set.rack }))
        .unwrap();
    set.shell
        .invoke(
            "device.set_param",
            json!({ "device": set.rack, "param": bl_model::macro_param(1), "value": 0.9 }),
        )
        .unwrap();
    set.shell
        .invoke(
            "rack.set_macro_variation_excluded",
            json!({ "rack": set.rack, "macro_number": 1, "excluded": true }),
        )
        .unwrap();
    set.shell
        .invoke(
            "rack.launch_variation",
            json!({ "rack": set.rack, "index": 0 }),
        )
        .unwrap();
    assert!(
        (macro_value(&set, 1) - 0.9).abs() < 1e-12,
        "the excluded macro kept its value"
    );
    // The variation still holds 0.5, so switching the exclusion off lets the
    // next launch set the macro.
    set.shell
        .invoke(
            "rack.set_macro_variation_excluded",
            json!({ "rack": set.rack, "macro_number": 1, "excluded": false }),
        )
        .unwrap();
    set.shell
        .invoke(
            "rack.launch_variation",
            json!({ "rack": set.rack, "index": 0 }),
        )
        .unwrap();
    assert!((macro_value(&set, 1) - 0.5).abs() < 1e-9);
}

/// A shell playing through a null device paced by hand, with a test clock, so
/// a recording test drives time itself rather than the wall clock (T199).
fn clocked() -> (Shell, NullControl) {
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell.use_test_clock(control.clock().expect("manual pacing"));
    shell.start_audio();
    (shell, control)
}

fn set_macro(shell: &mut Shell, rack: DeviceId, number: u8, value: f64) {
    shell
        .invoke(
            "device.set_param",
            json!({ "device": rack, "param": bl_model::macro_param(number), "value": value }),
        )
        .unwrap();
}

fn macro_value_of(shell: &Shell, rack: DeviceId, number: u8) -> f64 {
    shell
        .editor()
        .document()
        .device(rack)
        .and_then(|d| d.params.get(&bl_model::macro_param(number)).copied())
        .unwrap_or(0.0)
}

fn automation_enabled(shell: &Shell, track: TrackId, target: AutomationTarget) -> bool {
    shell
        .editor()
        .document()
        .track(track)
        .and_then(|track| track.automation_for(&target))
        .is_some_and(|automation| automation.enabled)
}

// Spec: macros R27
#[test]
fn a_launch_overrides_automated_macros_and_records_while_armed() {
    let (mut shell, control) = clocked();
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
    let (rack, utility) = (rack.device, utility.device);
    let macro_one = bl_model::macro_param(1);
    let target = AutomationTarget::Device {
        device: rack,
        param: macro_one,
    };
    // Macro 1 drives the Utility's Width. A variation holds 0.25, and the
    // macro stands at 0.75 with an envelope of its own.
    shell
        .invoke(
            "rack.map_macro",
            json!({
                "rack": rack,
                "macro_number": 1,
                "target": { "type": "device", "device": utility, "param": WIDTH },
            }),
        )
        .unwrap();
    set_macro(&mut shell, rack, 1, 0.25);
    shell
        .invoke("rack.store_variation", json!({ "rack": rack }))
        .unwrap();
    set_macro(&mut shell, rack, 1, 0.75);
    for at in [Ticks::ZERO, Ticks::WHOLE * 4] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({
                    "track": audio,
                    "target": { "type": "device", "device": rack, "param": macro_one },
                    "at": at,
                    "value": 0.75,
                }),
            )
            .unwrap();
    }
    testing::settle(&control, &mut shell);

    // Spec: macros R27. The launch sets the macro to the variation's value and
    // overrides the envelope it had.
    shell
        .invoke("rack.launch_variation", json!({ "rack": rack, "index": 0 }))
        .unwrap();
    assert!((macro_value_of(&shell, rack, 1) - 0.25).abs() < 1e-9);
    assert!(
        !automation_enabled(&shell, audio, target),
        "the launch overrode the macro's envelope"
    );

    // Spec: macros R27, automation R17. While Automation Arm records, the
    // launch is written as the macro's envelope too.
    shell
        .run("arrangement.automation_arm", json!({ "on": true }))
        .unwrap();
    shell
        .run("transport.set_recording", json!({ "recording": true }))
        .unwrap();
    shell.run("transport.play", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "recording", |shell| {
        let engine = shell.poll_engine();
        engine.recording && engine.playing
    });
    testing::advance(&control, &mut shell, 40);
    shell
        .invoke("rack.launch_variation", json!({ "rack": rack, "index": 0 }))
        .unwrap();
    shell.run("transport.stop", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the recording to end", |shell| {
        !shell.poll_engine().playing
    });
    let recorded = shell
        .editor()
        .document()
        .track(audio)
        .and_then(|track| track.automation_for(&target))
        .map(|automation| automation.envelope.clone())
        .expect("the recording made an envelope");
    assert!(
        recorded
            .points()
            .any(|point| (point.value - 0.25).abs() < 1e-9),
        "the launch was recorded"
    );
    assert!(
        shell
            .editor()
            .undo_history()
            .any(|label| label == "Record Automation"),
        "one undo step for the recording"
    );
}

//! The device frame's shared parts for the effects (WP-5.42; the device
//! chains spec, R34 and R35): a device's own items in its Show Options
//! menu, and the expand toggle's command. The device is the framework's
//! test effects device.

use bl_devices::testing::{self, EFFECTS_INVERT, INSTRUMENT_HI_QUALITY};
use bl_model::TrackId;
use bl_shell::device_commands::{
    DeviceAdded, DeviceParameters, DeviceViewState, frame_width, window_min_size,
};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

fn shell() -> Shell {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let audio: TrackId = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    shell
}

fn add(shell: &mut Shell, device_type: &bl_engine::DeviceType) -> DeviceAdded {
    let device =
        json!({ "kind": device_type.kind, "name": device_type.name, "role": device_type.role });
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

fn set_invert(shell: &mut Shell, device: &DeviceAdded, on: bool) {
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device.device, "param": EFFECTS_INVERT, "value": f64::from(u8::from(on)) }),
        )
        .unwrap();
}

/// The test effects' menu options as the frame lists them: name and tick.
fn options(shell: &Shell, index: usize) -> Vec<(String, bool)> {
    state(shell).devices[index]
        .menu_options()
        .iter()
        .map(|o| (o.name.to_owned(), o.on))
        .collect()
}

// Spec: device-chains R35
#[test]
fn a_devices_own_menu_option_is_ticked_after_save_as_default_preset_and_toggles_in_one_undo_step() {
    let library = std::env::temp_dir().join(format!("bl-effect-parts-{}", std::process::id()));
    std::fs::remove_dir_all(&library).ok();
    std::fs::create_dir_all(&library).unwrap();
    let mut shell = shell();
    shell.set_user_library(Some(library.clone()));
    let effects = testing::effects_type();
    let first = add(&mut shell, &effects);
    assert_eq!(options(&shell, 0), [("Invert LFO".to_owned(), false)]);
    // It has no control in the body: the layout places the others only.
    let panel = state(&shell).devices[0].panel.clone().unwrap();
    let placed = panel.layout(true).place(Default::default());
    assert!(placed.controls.iter().all(|c| c.param != EFFECTS_INVERT));
    assert_eq!(placed.controls.len(), panel.description.params.len() - 1);

    // One undo step to turn it on, named for the change; it reads back.
    let steps = shell.editor().undo_history().len();
    set_invert(&mut shell, &first, true);
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(options(&shell, 0), [("Invert LFO".to_owned(), true)]);

    // Saved as the default, a new device starts with it ticked.
    shell
        .invoke("device.save_default_preset", json!({}))
        .unwrap();
    let second = add(&mut shell, &effects);
    assert_eq!(options(&shell, 1), [("Invert LFO".to_owned(), true)]);

    // Turning it off is one step too, and undo turns it back on.
    let steps = shell.editor().undo_history().len();
    set_invert(&mut shell, &second, false);
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(options(&shell, 1), [("Invert LFO".to_owned(), false)]);
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(options(&shell, 1), [("Invert LFO".to_owned(), true)]);

    // Agents read it like any parameter, and it can be automated (G2).
    let reply: Value = shell
        .run("device.parameters", json!({ "device": second.device }))
        .unwrap();
    let parameters: DeviceParameters = serde_json::from_value(reply).unwrap();
    let invert = parameters.params.last().unwrap();
    assert_eq!(invert.descriptor.name, "Invert LFO");
    assert!(invert.menu_option && invert.descriptor.automatable);
    assert_eq!(invert.value, 1.0);
    assert!(
        parameters.params[..parameters.params.len() - 1]
            .iter()
            .all(|p| !p.menu_option)
    );
    std::fs::remove_dir_all(&library).ok();
}

// Spec: device-chains R34
#[test]
fn the_expand_toggle_widens_the_device_like_fold_and_changes_only_the_view() {
    let mut shell = shell();
    let effects = add(&mut shell, &testing::effects_type());
    let scope = add(&mut shell, &testing::scope_type());
    let steps = shell.editor().undo_history().len();

    let frames = state(&shell).devices;
    assert!(frames[0].expandable && !frames[0].expanded);
    assert!(!frames[1].expandable, "the scope has no expanded part");
    let closed = frame_width(&frames[0]);

    // On a device with an expanded part it shows and hides it.
    assert!(shell.can_invoke("device.expand", &json!({ "device": effects.device })));
    shell
        .invoke("device.expand", json!({ "device": effects.device }))
        .unwrap();
    let frames = state(&shell).devices;
    assert!(frames[0].expanded);
    let layout = &frames[0].panel.as_ref().unwrap().description.layout;
    let part = layout.shown(true).docked_size().width - layout.shown(false).docked_size().width;
    assert!(part > 0.0);
    assert_eq!(frame_width(&frames[0]), closed + part);
    assert_eq!(window_min_size(&frames[0]).0, closed + part);
    shell
        .invoke(
            "device.expand",
            json!({ "device": effects.device, "expanded": false }),
        )
        .unwrap();
    assert!(!state(&shell).devices[0].expanded);
    assert_eq!(frame_width(&state(&shell).devices[0]), closed);

    // It isn't an undo step, and a device without the part refuses it.
    assert_eq!(shell.editor().undo_history().len(), steps);
    assert!(!shell.can_invoke("device.expand", &json!({ "device": scope.device })));
    assert!(
        shell
            .invoke("device.expand", json!({ "device": scope.device }))
            .is_err()
    );
    assert!(!state(&shell).devices[1].expanded);
}

// Spec: instrument voices R19
#[test]
fn an_instruments_hi_quality_is_a_menu_option_that_one_undo_step_switches_and_agents_read_back() {
    let mut shell = shell();
    let instrument = add(&mut shell, &testing::instrument_type());
    let hi_quality = |shell: &mut Shell| -> (bool, bool, f64) {
        let reply: Value = shell
            .run("device.parameters", json!({ "device": instrument.device }))
            .unwrap();
        let parameters: DeviceParameters = serde_json::from_value(reply).unwrap();
        let param = parameters
            .params
            .iter()
            .find(|p| p.descriptor.id == INSTRUMENT_HI_QUALITY)
            .unwrap();
        (param.menu_option, param.descriptor.automatable, param.value)
    };
    // Off in a new device, a menu option, automatable like any parameter.
    assert_eq!(hi_quality(&mut shell), (true, true, 0.0));
    assert_eq!(options(&shell, 0), [("Hi-Quality".to_owned(), false)]);

    let steps = shell.editor().undo_history().len();
    shell
        .invoke(
            "device.set_param",
            json!({ "device": instrument.device, "param": INSTRUMENT_HI_QUALITY, "value": 1.0 }),
        )
        .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(hi_quality(&mut shell).2, 1.0);
    assert_eq!(options(&shell, 0), [("Hi-Quality".to_owned(), true)]);

    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(hi_quality(&mut shell).2, 0.0);
}

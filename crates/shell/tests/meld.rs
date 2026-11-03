//! Meld in the shell (WP-8.17; the meld spec, R3, R5 and R19): its macros
//! are named by their oscillator's type and keep their IDs, its Semitones
//! read in scale degrees while Use Current Scale is on, and its engine
//! selector is view state. Driven through `Shell::run`, as an agent drives
//! it.

use bl_devices::meld::{
    FILTER_MACRO_1, FILTER_TYPE, MACRO_1, MACRO_2, MELD, SEMITONES, TYPE, USE_SCALE, param,
};
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use serde_json::json;

fn midi_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap()
}

fn add_meld(shell: &mut Shell) -> DeviceId {
    let track = midi_track(shell);
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: MELD.to_owned() },
                    "name": "Meld",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn set(shell: &mut Shell, device: DeviceId, id: bl_model::ParamId, value: f64) {
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        )
        .unwrap();
}

fn read(shell: &mut Shell, device: DeviceId) -> DeviceParameters {
    serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: meld R3, R5
#[test]
fn the_macros_are_named_by_the_type_and_semitones_read_in_scale_degrees() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    let name = |read: &DeviceParameters, id| {
        let p = read.params.iter().find(|p| p.descriptor.id == id).unwrap();
        (p.descriptor.name.clone(), p.text.clone())
    };
    let first = read(&mut shell, device);
    assert_eq!(name(&first, param(0, MACRO_1)).0, "Shape");
    assert_eq!(name(&first, param(0, MACRO_2)).0, "Tone");
    assert_eq!(name(&first, param(0, FILTER_MACRO_1)).0, "Q");
    // Square Sync names its macros Freq 1 and Freq 2, with the same IDs and
    // values.
    set(&mut shell, device, param(0, MACRO_1), 40.0);
    set(&mut shell, device, param(0, TYPE), 3.0);
    let second = read(&mut shell, device);
    assert_eq!(
        name(&second, param(0, MACRO_1)),
        ("Freq 1".to_owned(), "40 %".to_owned())
    );
    assert_eq!(name(&second, param(0, MACRO_2)).0, "Freq 2");
    assert_eq!(
        name(&second, param(1, MACRO_1)).0,
        "Shape",
        "B keeps its own type"
    );
    // The filter's macros follow its type.
    set(&mut shell, device, param(1, FILTER_TYPE), 8.0);
    assert_eq!(
        name(&read(&mut shell, device), param(1, FILTER_MACRO_1)).0,
        "Boost"
    );
    // Semitones read in degrees while Use Current Scale is on.
    set(&mut shell, device, param(0, SEMITONES), 2.0);
    assert_eq!(
        name(&read(&mut shell, device), param(0, SEMITONES)).1,
        "2 st"
    );
    set(&mut shell, device, USE_SCALE, 1.0);
    assert_eq!(
        name(&read(&mut shell, device), param(0, SEMITONES)).1,
        "2 sd"
    );
}

// Spec: meld R19
#[test]
fn the_engine_selector_is_view_state_that_shows_one_engines_controls() {
    let mut shell = Shell::headless();
    let device = add_meld(&mut shell);
    let history = |shell: &Shell| -> Vec<String> {
        shell.editor().undo_history().map(str::to_owned).collect()
    };
    let (steps, dirty) = (history(&shell), shell.is_dirty());
    let reply: serde_json::Value = shell
        .invoke(
            "device.set_page",
            json!({ "device": device, "node": "Engine", "page": "B" }),
        )
        .unwrap();
    assert_eq!(reply["devices"][0]["panel"]["pages"][0]["shown"], "B");
    assert_eq!(history(&shell), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "and the set is unchanged");
}

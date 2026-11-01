//! Operator in the shell (WP-8.12; the Operator spec, R11, R18, R19): the
//! browser lists it under Instruments, its shell's sections are pages that
//! `device.set_page` switches, and a set with Operator saves and reopens
//! with every parameter and its section.

use std::path::PathBuf;

use bl_devices::operator::{
    ALGORITHM, ENV_ATTACK, NOTE_PB, OPERATOR, OSC_FINE, operator_type, osc,
};
use bl_model::{DeviceId, DeviceKind, DeviceRole, ParamId};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::browser_commands::{self as ids, BrowserState};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

const NODE: &str = "Sections";

fn add(shell: &mut Shell) -> DeviceId {
    shell.set_device_types(bl_devices::device_types());
    let audio = shell.editor().document().top_level().nth(2).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let t = operator_type();
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

fn shown(shell: &Shell, device: DeviceId) -> String {
    let frame = shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap();
    frame
        .panel
        .unwrap()
        .pages
        .into_iter()
        .find(|p| p.node == NODE)
        .unwrap()
        .shown
}

/// The parameters whose controls the body places now.
fn placed(shell: &Shell, device: DeviceId) -> Vec<u32> {
    let frame = shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap();
    let layout = frame.panel.unwrap().layout(false);
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| c.param.0)
        .collect()
}

fn set_page(shell: &mut Shell, device: DeviceId, page: &str) {
    shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": NODE, "page": page }),
        )
        .unwrap();
}

// Spec: operator R11
#[test]
fn the_browser_lists_operator_under_instruments_as_an_instrument() {
    let mut shell = Shell::headless();
    let state: BrowserState = serde_json::from_value(
        shell
            .run(ids::OPEN_PLACE, json!({ "label": "instruments" }))
            .unwrap(),
    )
    .unwrap();
    let row = state
        .rows
        .iter()
        .find(|row| row.name == "Operator")
        .expect("Operator is listed");
    assert_eq!(row.kind, bl_shell_spec::browser::ItemKind::Device);
    assert_eq!(operator_type().role, DeviceRole::Instrument);
    assert_eq!(
        operator_type().kind,
        DeviceKind::Native {
            device_type: OPERATOR.to_owned()
        }
    );
}

// Spec: operator R18
#[test]
fn a_sections_button_switches_the_display_and_every_parameter_stays_reachable() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    let panel = shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .unwrap();
    assert_eq!(panel.pages.len(), 1);
    assert_eq!(
        panel.pages[0].style,
        bl_model::device_layout::PageStyle::Shell
    );
    assert_eq!(
        panel.pages[0].pages,
        [
            "Global", "Osc A", "Osc B", "Osc C", "Osc D", "LFO", "Filter", "Pitch"
        ]
    );
    assert_eq!(shown(&shell, device), "Global");
    // The Global section's display holds the global controls; the
    // oscillators' further controls aren't placed until their section shows.
    let global = placed(&shell, device);
    assert!(global.contains(&ALGORITHM.0));
    assert!(!global.contains(&osc(2, OSC_FINE).0));
    set_page(&mut shell, device, "Osc C");
    let c = placed(&shell, device);
    assert_eq!(shown(&shell, device), "Osc C");
    assert!(c.contains(&osc(2, OSC_FINE).0) && c.contains(&osc(2, ENV_ATTACK).0));
    assert!(!c.contains(&osc(1, OSC_FINE).0));
    // The sections' own controls show whichever section does.
    assert!(c.contains(&ALGORITHM.0));
    // Switching is no undo step, and a section the device lacks is refused.
    let steps = shell.editor().undo_history().len();
    set_page(&mut shell, device, "Pitch");
    assert_eq!(shell.editor().undo_history().len(), steps);
    assert!(
        shell
            .invoke(
                "device.set_page",
                json!({ "device": device, "node": NODE, "page": "Oscillators" }),
            )
            .is_err()
    );
    // A parameter on a hidden section is set and read like a shown one.
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": osc(0, OSC_FINE), "value": 250.0 }),
        )
        .unwrap();
    let held = shell.editor().document().device(device).unwrap().params[&osc(0, OSC_FINE)];
    assert_eq!(held, 250.0);
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-operator-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Spec: operator R19
#[test]
fn a_set_with_operator_saves_and_reopens_with_every_parameter_and_its_section() {
    let scratch = Scratch::new();
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let device = add(&mut shell);
    // Every parameter off its default, to the other end of its range.
    let mut wanted: Vec<(ParamId, f64)> = Vec::new();
    for param in &operator_type().description.params {
        let to = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        shell
            .invoke(
                "device.set_param",
                json!({ "device": device, "param": param.id, "value": to }),
            )
            .unwrap();
        wanted.push((param.id, to));
    }
    set_page(&mut shell, device, "Osc C");
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": scratch.0.join("Operator.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    let path = saved.path.expect("the set was saved");
    set_page(&mut shell, device, "Pitch");
    shell.run("set.open", json!({ "path": path })).unwrap();
    let audio = shell.editor().document().device_track(device).unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let reopened = shell.editor().document().device(device).unwrap().clone();
    for (param, value) in wanted {
        assert_eq!(reopened.params.get(&param), Some(&value), "{param:?}");
    }
    assert_eq!(reopened.params.get(&NOTE_PB), Some(&0.0));
    assert_eq!(shown(&shell, device), "Osc C");
}

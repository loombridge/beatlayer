//! Operator in the shell (WP-8.12; the Operator spec, R11, R18, R19): the
//! browser lists it under Instruments, its shell's sections are pages that
//! `device.set_page` switches, and a set with Operator saves and reopens
//! with every parameter and its section.

use std::path::PathBuf;

use bl_devices::operator::{
    ALGORITHM, ENV_ATTACK, NOTE_PB, OPERATOR, OSC_FINE, OSC_REPEAT, OSC_REPEAT_ON, OSC_WAVE,
    SHAPED_INITIAL, SHAPED_PEAK, SHAPED_SUSTAIN, operator_type, osc,
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

// ---- Part 2 (WP-8.13): the menu's options, drawn partials and the displays ----

fn set(shell: &mut Shell, device: DeviceId, param: ParamId, value: f64) {
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
}

fn value(shell: &Shell, device: DeviceId, param: ParamId) -> f64 {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or_else(|| {
            operator_type()
                .description
                .param(param)
                .expect("a parameter")
                .default
        })
}

fn partials(shell: &Shell, device: DeviceId, op: u8) -> Option<Vec<f32>> {
    shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .partials
        .iter()
        .find(|p| p.osc == op)
        .map(|p| p.levels.clone())
}

// Spec: operator R27 (device.set_partials: levels, count and Repeat, one undo step)
#[test]
fn set_partials_sets_levels_count_and_repeat_as_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    let mut levels = vec![0.0f32; 32];
    levels[0] = 1.0;
    levels[2] = 0.5;
    let result = shell
        .run(
            "device.set_partials",
            json!({ "device": device, "osc": 1, "levels": levels, "repeat": 40.0 }),
        )
        .unwrap();
    assert_eq!(result, json!({ "device": device, "osc": 1, "count": 32 }));
    assert_eq!(partials(&shell, device, 1), Some(levels.clone()));
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT)), 40.0);
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT_ON)), 1.0);
    // Repeat can be switched off by itself.
    shell
        .run(
            "device.set_partials",
            json!({ "device": device, "osc": 1, "levels": levels, "repeat_on": false }),
        )
        .unwrap();
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT_ON)), 0.0);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT_ON)), 1.0);
    // One undo step puts levels, count and Repeat back.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(partials(&shell, device, 1), None);
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT)), 0.0);
    assert_eq!(value(&shell, device, osc(1, OSC_REPEAT_ON)), 0.0);
    // A count that isn't 16, 32 or 64, a level out of range and an
    // oscillator that isn't there are refused.
    for (osc, levels) in [(0, vec![1.0; 20]), (0, vec![1.5; 16]), (4, vec![0.5; 16])] {
        assert!(
            shell
                .run(
                    "device.set_partials",
                    json!({ "device": device, "osc": osc, "levels": levels }),
                )
                .is_err()
        );
    }
    assert_eq!(partials(&shell, device, 0), None);
}

// Spec: operator R28 (Copy Oscillator settings)
#[test]
fn copy_oscillator_sends_every_setting_and_the_partials_to_another_in_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    set(&mut shell, device, osc(0, OSC_FINE), 321.0);
    set(&mut shell, device, osc(0, ENV_ATTACK), 777.0);
    shell
        .run(
            "device.set_partials",
            json!({ "device": device, "osc": 0, "levels": vec![0.25f32; 16] }),
        )
        .unwrap();
    let copied = shell
        .run(
            "operator.copy_oscillator",
            json!({ "device": device, "from": 0, "into": 2 }),
        )
        .unwrap();
    assert_eq!(copied, json!({ "device": device, "from": 0, "into": 2 }));
    assert_eq!(value(&shell, device, osc(2, OSC_FINE)), 321.0);
    assert_eq!(value(&shell, device, osc(2, ENV_ATTACK)), 777.0);
    assert_eq!(partials(&shell, device, 2), Some(vec![0.25; 16]));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(value(&shell, device, osc(2, OSC_FINE)), 0.0);
    assert_eq!(partials(&shell, device, 2), None);
    assert!(
        shell
            .run(
                "operator.copy_oscillator",
                json!({ "device": device, "from": 0, "into": 4 })
            )
            .is_err()
    );
}

// Spec: operator R28 (Set All Envelope Levels, Play By Key)
#[test]
fn the_menus_envelope_levels_and_play_by_key_set_their_parameters_in_one_undo_step() {
    use bl_devices::operator::{
        ENV_INITIAL, ENV_PEAK, ENV_SUSTAIN, FILTER_ENV, FILTER_FREQ, FREQ_KEY, SHAPED_END, shaped,
    };
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    for (to, amplitude, shaped_level) in [
        ("maximum", 1.0, 100.0),
        ("minimum", 0.0, -100.0),
        ("middle", 0.5, 0.0),
    ] {
        shell
            .run(
                "operator.set_envelope_levels",
                json!({ "device": device, "envelope": "B", "to": to }),
            )
            .unwrap();
        for control in [ENV_INITIAL, ENV_PEAK, ENV_SUSTAIN] {
            assert_eq!(value(&shell, device, osc(1, control)), amplitude, "{to}");
        }
        shell
            .run(
                "operator.set_envelope_levels",
                json!({ "device": device, "envelope": "Filter", "to": to }),
            )
            .unwrap();
        for control in [SHAPED_INITIAL, SHAPED_PEAK, SHAPED_SUSTAIN, SHAPED_END] {
            assert_eq!(
                value(&shell, device, shaped(FILTER_ENV, control)),
                shaped_level,
                "{to}"
            );
        }
    }
    assert!(
        shell
            .run(
                "operator.set_envelope_levels",
                json!({ "device": device, "envelope": "E", "to": "middle" })
            )
            .is_err()
    );
    shell
        .run("operator.play_by_key", json!({ "device": device }))
        .unwrap();
    assert_eq!(value(&shell, device, FILTER_FREQ), 466.0);
    assert_eq!(value(&shell, device, FREQ_KEY), 100.0);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(value(&shell, device, FILTER_FREQ), 20_000.0);
    assert_eq!(value(&shell, device, FREQ_KEY), 0.0);
}

// Spec: operator R27 (the harmonics editor's options are view state)
#[test]
fn even_only_and_odd_only_exclude_each_other_and_normalize_starts_on() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    let options = |shell: &mut Shell, args| -> serde_json::Value {
        shell.run("operator.set_harmonics", args).unwrap()
    };
    let new = options(&mut shell, json!({ "device": device }));
    assert_eq!(
        new,
        json!({ "device": device, "even_only": false, "odd_only": false, "normalize": true })
    );
    let even = options(&mut shell, json!({ "device": device, "even_only": true }));
    assert_eq!(even["even_only"], json!(true));
    let odd = options(
        &mut shell,
        json!({ "device": device, "odd_only": true, "normalize": false }),
    );
    assert_eq!(
        (
            odd["even_only"].clone(),
            odd["odd_only"].clone(),
            odd["normalize"].clone()
        ),
        (json!(false), json!(true), json!(false))
    );
}

// Spec: operator R29 (device.display, headless)
#[test]
fn device_display_reports_the_view_a_section_starts_on_from_the_parameters() {
    let mut shell = Shell::headless();
    let device = add(&mut shell);
    let display = |shell: &mut Shell| -> serde_json::Value {
        shell
            .invoke("device.display", json!({ "device": device }))
            .unwrap()
    };
    // Global's page starts on the algorithm graph.
    let global = display(&mut shell);
    assert_eq!(global["view"], json!("Algorithm"));
    assert_eq!(global["readings"][0]["name"], json!("Algorithm"));
    // The LFO's page shows its envelope, the filter's the filter's.
    set_page(&mut shell, device, "LFO");
    let lfo = display(&mut shell);
    assert_eq!(lfo["view"], json!("LFO Envelope"));
    assert_eq!(lfo["series"][0]["name"], json!("LFO Envelope"));
    set_page(&mut shell, device, "Filter");
    assert_eq!(
        display(&mut shell)["series"][0]["name"],
        json!("Filter Envelope")
    );
    shell
        .run(
            "device.set_display_view",
            json!({ "device": device, "view": "Filter Response" }),
        )
        .unwrap();
    let response = display(&mut shell);
    assert_eq!(response["series"][0]["name"], json!("Filter Response"));
    assert_eq!(
        response["series"][0]["values"].as_array().unwrap().len(),
        96
    );
    // An oscillator on User shows its partials.
    set_page(&mut shell, device, "Osc A");
    set(&mut shell, device, osc(0, OSC_WAVE), 22.0);
    let mut levels = vec![0.0f32; 16];
    levels[1] = 0.8;
    shell
        .run(
            "device.set_partials",
            json!({ "device": device, "osc": 0, "levels": levels }),
        )
        .unwrap();
    shell
        .run(
            "device.set_display_view",
            json!({ "device": device, "view": "Osc A Oscillator" }),
        )
        .unwrap();
    let wave = display(&mut shell);
    let partials = wave["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == json!("Partials"))
        .expect("the partials");
    assert_eq!(partials["values"][1], json!(0.8f32));
    assert_eq!(partials["values"].as_array().unwrap().len(), 16);
}

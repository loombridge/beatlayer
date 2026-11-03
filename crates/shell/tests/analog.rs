//! Analog in the shell (WP-8.15; the Analog spec, R22, R26 and R27): Quick
//! Routing's four buttons set the routing as one undo step each and change no
//! oscillator's level or tuning; the shell's sections switch without an undo
//! step; and `device.display` reports each section's drawing. Driven through
//! `Shell::invoke`, as an agent drives it.

use bl_devices::analog::{
    self, ANALOG, SECTIONS, amp, env, filter, lfo, noise, osc,
};
use bl_model::{DeviceDisplay, DeviceId, DeviceKind, DeviceRole, ParamId, TrackKind};
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use bl_shell::{Area, Shell};
use serde_json::json;

fn add(shell: &mut Shell, device_type: &str) -> DeviceId {
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: device_type.to_owned() },
                    "name": "Device",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn set(shell: &mut Shell, device: DeviceId, id: ParamId, value: f64) {
    shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        )
        .unwrap();
}

fn read(shell: &mut Shell, device: DeviceId, id: ParamId) -> f64 {
    let read: DeviceParameters = serde_json::from_value(
        shell
            .invoke("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    read.params
        .iter()
        .find(|p| p.descriptor.id == id)
        .unwrap()
        .value
}

fn history(shell: &Shell) -> Vec<String> {
    shell.editor().undo_history().map(str::to_owned).collect()
}

/// The values the routing parameters have: both filters' and amplifiers'
/// switches, To F2, the amplifiers' pans, and the three balances.
fn state(shell: &mut Shell, device: DeviceId) -> Vec<(&'static str, f64)> {
    [
        ("Filter 1", filter::id(0, filter::ON)),
        ("Filter 2", filter::id(1, filter::ON)),
        ("Amp 1", amp::id(0, amp::ON)),
        ("Amp 2", amp::id(1, amp::ON)),
        ("To F2", filter::id(0, filter::EXTRA)),
        ("Pan 1", amp::id(0, amp::PAN)),
        ("Pan 2", amp::id(1, amp::PAN)),
        ("Osc 1 F1/F2", osc::id(0, osc::BALANCE)),
        ("Osc 2 F1/F2", osc::id(1, osc::BALANCE)),
        ("Noise F1/F2", noise::id(noise::BALANCE)),
    ]
    .into_iter()
    .map(|(name, id)| (name, read(shell, device, id)))
    .collect()
}

/// The oscillators' levels and tunings and the noise's level.
fn tuning(shell: &mut Shell, device: DeviceId) -> Vec<f64> {
    let mut ids = Vec::new();
    for n in 0..2 {
        for offset in [
            osc::LEVEL,
            osc::OCTAVE,
            osc::SEMI,
            osc::DETUNE,
            osc::KEY,
            osc::ON,
        ] {
            ids.push(osc::id(n, offset));
        }
    }
    ids.push(noise::id(noise::LEVEL));
    ids.into_iter().map(|id| read(shell, device, id)).collect()
}

const ON: f64 = 1.0;
const OFF: f64 = 0.0;

// Spec: analog R22
#[test]
fn each_quick_routing_button_sets_its_routing_as_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, ANALOG);
    // Settings that every routing overrides or leaves, and an oscillator
    // level and tuning that none may touch.
    for (id, value) in [
        (osc::id(0, osc::LEVEL), 0.4),
        (osc::id(1, osc::LEVEL), 0.7),
        (osc::id(1, osc::OCTAVE), 1.0),
        (osc::id(0, osc::SEMI), 5.0),
        (osc::id(1, osc::DETUNE), -40.0),
        (noise::id(noise::LEVEL), 0.3),
        (filter::id(1, filter::EXTRA), 1.0),
        (filter::id(0, filter::EXTRA), 40.0),
        (osc::id(0, osc::BALANCE), 80.0),
        (noise::id(noise::BALANCE), 60.0),
    ] {
        set(&mut shell, device, id, value);
    }
    let tuned = tuning(&mut shell, device);
    let routings = [
        (
            "parallel",
            [
                ("Filter 1", ON),
                ("Filter 2", ON),
                ("Amp 1", ON),
                ("Amp 2", ON),
                ("To F2", 0.0),
                ("Pan 1", 0.0),
                ("Pan 2", 0.0),
                ("Osc 1 F1/F2", 0.0),
                ("Osc 2 F1/F2", 100.0),
                ("Noise F1/F2", 0.0),
            ],
        ),
        (
            "split",
            [
                ("Filter 1", ON),
                ("Filter 2", ON),
                ("Amp 1", ON),
                ("Amp 2", ON),
                ("To F2", 0.0),
                ("Pan 1", -100.0),
                ("Pan 2", 100.0),
                ("Osc 1 F1/F2", 0.0),
                ("Osc 2 F1/F2", 100.0),
                ("Noise F1/F2", 0.0),
            ],
        ),
        (
            "filter_1_only",
            [
                ("Filter 1", ON),
                ("Filter 2", OFF),
                ("Amp 1", ON),
                ("Amp 2", OFF),
                ("To F2", 0.0),
                ("Pan 1", -100.0),
                ("Pan 2", 100.0),
                ("Osc 1 F1/F2", 0.0),
                ("Osc 2 F1/F2", 0.0),
                ("Noise F1/F2", 0.0),
            ],
        ),
        (
            "serial",
            [
                ("Filter 1", ON),
                ("Filter 2", ON),
                ("Amp 1", OFF),
                ("Amp 2", ON),
                ("To F2", 100.0),
                ("Pan 1", -100.0),
                ("Pan 2", 100.0),
                ("Osc 1 F1/F2", 0.0),
                ("Osc 2 F1/F2", 0.0),
                ("Noise F1/F2", 0.0),
            ],
        ),
    ];
    for (name, expected) in routings {
        let before = state(&mut shell, device);
        let steps = history(&shell);
        let reply = shell
            .invoke(
                "analog.quick_routing",
                json!({ "device": device, "routing": name }),
            )
            .unwrap();
        assert_eq!(reply, json!({ "device": device, "routing": name }));
        let after = state(&mut shell, device);
        // Pans a routing doesn't name stay as they were.
        let wanted: Vec<(&str, f64)> = expected
            .iter()
            .map(|&(what, value)| {
                let kept = matches!(
                    (name, what),
                    ("filter_1_only" | "serial", "Pan 1" | "Pan 2")
                );
                let value = if kept {
                    before.iter().find(|(n, _)| *n == what).unwrap().1
                } else {
                    value
                };
                (what, value)
            })
            .collect();
        let wanted: Vec<(&str, f64)> = wanted
            .into_iter()
            .map(|(what, value)| {
                // Filter 1 Only leaves To F2 where it was: nothing reaches Filter 2.
                if name == "filter_1_only" && what == "To F2" {
                    (what, before.iter().find(|(n, _)| *n == what).unwrap().1)
                } else {
                    (what, value)
                }
            })
            .collect();
        assert_eq!(after, wanted, "{name}");
        assert_eq!(tuning(&mut shell, device), tuned, "{name} changes no level or tuning");
        // One undo step, named for the button.
        let now = history(&shell);
        assert_eq!(now.len(), steps.len() + 1, "{name}: {now:?}");
        assert_eq!(now[0], "Quick Routing");
        shell.invoke("edit.undo", json!({})).unwrap();
        assert_eq!(state(&mut shell, device), before, "{name}: one undo puts it back");
        assert_eq!(history(&shell), steps);
        shell.invoke("edit.redo", json!({})).unwrap();
    }
}

// Spec: analog R22
#[test]
fn quick_routing_is_refused_for_a_device_that_isnt_an_analog() {
    let mut shell = Shell::headless();
    let meld = add(&mut shell, bl_devices::meld::MELD);
    let steps = history(&shell);
    assert!(
        shell
            .invoke(
                "analog.quick_routing",
                json!({ "device": meld, "routing": "parallel" })
            )
            .is_err()
    );
    assert!(
        shell
            .invoke(
                "analog.quick_routing",
                json!({ "device": meld, "routing": "diagonal" })
            )
            .is_err(),
        "an unknown routing"
    );
    assert_eq!(history(&shell), steps);
}

fn display(shell: &mut Shell, device: DeviceId) -> DeviceDisplay {
    serde_json::from_value(
        shell
            .invoke("device.display", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap()
}

// Spec: analog R26
#[test]
fn an_agent_reads_every_sections_drawing_headless() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, ANALOG);
    let shown = display(&mut shell, device);
    assert!(!shown.live, "no engine: what the parameters give");
    for name in analog::display::ENVELOPES
        .iter()
        .chain(&analog::display::RESPONSES)
        .chain(&analog::display::WAVES)
    {
        assert!(shown.series(name).is_some(), "{name}");
    }
    // The drawing follows the parameters: a longer attack, a higher cutoff.
    let attack = |shell: &mut Shell| {
        let d = display(shell, device);
        let series = d.series("Amp 1 Envelope").unwrap();
        series.values.iter().position(|&v| v >= 0.999).unwrap() as f32 / series.values.len() as f32
    };
    let before = attack(&mut shell);
    set(&mut shell, device, env::id(2, env::ATTACK), 2_000.0);
    assert!(attack(&mut shell) > before, "a longer attack draws a longer rise");
    let cutoff = |shell: &mut Shell| {
        let d = display(shell, device);
        let series = d.series("Filter 1 Response").unwrap();
        series.values.iter().position(|&v| v < -3.0).unwrap_or(series.values.len())
    };
    set(&mut shell, device, filter::id(0, filter::FREQ), 200.0);
    let low = cutoff(&mut shell);
    set(&mut shell, device, filter::id(0, filter::FREQ), 5_000.0);
    assert!(cutoff(&mut shell) > low, "a higher cutoff draws the response further right");
    set(&mut shell, device, lfo::id(0, lfo::WAVE), 2.0);
    let wave = display(&mut shell, device);
    assert_eq!(wave.series("LFO 1 Wave").unwrap().values[0], 1.0, "a rectangle starts high");
}

// Spec: analog R27
#[test]
fn selecting_a_section_switches_the_display_and_is_no_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, ANALOG);
    let steps = history(&shell);
    let dirty = shell.is_dirty();
    let mut shown = Vec::new();
    for name in SECTIONS {
        shell
            .invoke(
                "device.set_page",
                json!({ "device": device, "node": analog::PAGES, "page": name }),
            )
            .unwrap();
        let frame = shell
            .device_view_state()
            .devices
            .into_iter()
            .find(|d| d.id == device)
            .unwrap();
        let panel = frame.panel.unwrap();
        assert_eq!(panel.pages[0].shown, name);
        let layout = panel.layout(false);
        let mut cells: Vec<String> = layout
            .place(layout.docked_size())
            .cells
            .iter()
            .map(|c| c.name.clone())
            .collect();
        cells.sort();
        shown.push((name, cells));
    }
    assert_eq!(history(&shell), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "the set is unchanged");
    let cells = |section: &str| &shown.iter().find(|(n, _)| *n == section).unwrap().1;
    assert_eq!(cells("Filter 2"), &["Filter 2 Envelope", "Filter 2 Response"]);
    assert_eq!(cells("Amplifier 1"), &["Amp 1 Envelope"]);
    assert_eq!(cells("LFO 2"), &["LFO 2 Wave"]);
    assert_eq!(cells("Global"), &["Quick Routing"]);
    assert!(cells("Oscillator 1").is_empty());
}

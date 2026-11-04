//! The synthesisers' panels as Live lays them out, part 1 (layout L-DEV-102
//! to L-DEV-105): each holds the controls Live's panel shows, in its order
//! and kinds, and none else, and a new device opens at the values and in the
//! units Live shows (observed in Live 12.4.16, 2 and 3 November 2026).

use bl_devices::device_types;
use bl_model::ParamId;
use bl_model::device_layout::{PageChoice, Placed, Widget};

/// A panel's controls by name, in order, with their kinds.
type Controls = Vec<(String, Widget)>;

/// The placed panel of the device named `name` at its docked size, showing
/// the pages `shown` names (node, page).
fn placed(name: &str, shown: &[(&str, &str)]) -> Placed {
    let types = device_types();
    let device = types
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} is a device"));
    let choice: PageChoice = shown
        .iter()
        .map(|(node, page)| ((*node).to_owned(), (*page).to_owned()))
        .collect();
    let layout = device.description.layout.with_pages(&choice);
    layout.place(layout.docked_size())
}

/// The panel's controls in the layout's order, as name and kind.
fn controls(name: &str, shown: &[(&str, &str)]) -> Vec<(String, Widget)> {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).expect("a device");
    placed(name, shown)
        .controls
        .iter()
        .map(|c| {
            let param = device
                .description
                .param(c.param)
                .unwrap_or_else(|| panic!("{name} has parameter {}", c.param.0));
            (param.name.clone(), c.widget)
        })
        .collect()
}

fn of(list: &[(&str, Widget)]) -> Vec<(String, Widget)> {
    list.iter().map(|(n, w)| ((*n).to_owned(), *w)).collect()
}

/// A parameter of the device named `name`, by its name.
fn param(name: &str, param: &str) -> (ParamId, bl_model::ParamDescriptor) {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).expect("a device");
    let found = device
        .description
        .params
        .iter()
        .find(|p| p.name == param)
        .unwrap_or_else(|| panic!("{name} has {param}"));
    (found.id, found.clone())
}

/// How a new device shows `param`: its default as the control reads it.
fn shown(name: &str, parameter: &str) -> String {
    let (_, p) = param(name, parameter);
    p.format(p.default)
}

const TOP: [(&str, Widget); 7] = [
    ("Stiffness", Widget::SmallKnob),
    ("Noise", Widget::SmallKnob),
    ("Tine", Widget::SmallKnob),
    ("Tone", Widget::SmallKnob),
    ("Pickup Symmetry", Widget::SmallKnob),
    ("Pickup Type", Widget::Stack(2)),
    ("Volume", Widget::SmallKnob),
];

/// Electric's panel with `page` shown: the sections across the top, then the
/// page's table.
fn electric(page: &str, table: &[(&str, Widget)]) -> (Controls, Controls) {
    let mut expected = TOP.to_vec();
    expected.extend_from_slice(table);
    (controls("Electric", &[("Sections", page)]), of(&expected))
}

// Spec: electric R16
#[test]
fn electric_holds_lives_controls_in_lives_order_on_each_page() {
    use Widget::{Unboxed, UnboxedChooser};
    let (got, want) = electric(
        "Hammer",
        &[
            ("Stiffness Vel", Unboxed),
            ("Stiffness Key", Unboxed),
            ("Force Amount", Unboxed),
            ("Force Vel", Unboxed),
            ("Force Key", Unboxed),
            ("Noise Pitch", Unboxed),
            ("Noise Decay", Unboxed),
            ("Noise Key", Unboxed),
        ],
    );
    assert_eq!(got, want);
    let (got, want) = electric(
        "Fork",
        &[
            ("Tine Color", Unboxed),
            ("Tine Decay", Unboxed),
            ("Tine Key", Unboxed),
            ("Release", Unboxed),
            ("Tone Decay", Unboxed),
        ],
    );
    assert_eq!(got, want);
    let (got, want) = electric(
        "Damper/Pickup",
        &[
            ("Damper Tone", Unboxed),
            ("Damper Att/Rel", Unboxed),
            ("Damper Level", Unboxed),
            ("Pickup Input", Unboxed),
            ("Pickup Output", Unboxed),
            ("Pickup Out Key", Unboxed),
            ("Pickup Distance", Unboxed),
        ],
    );
    assert_eq!(got, want);
    let (got, want) = electric(
        "Global",
        &[
            ("Voices", UnboxedChooser),
            ("Semi", Unboxed),
            ("Detune", Unboxed),
            ("Stretch", Unboxed),
            ("PB Range", Unboxed),
            ("Note PB", Unboxed),
        ],
    );
    assert_eq!(got, want);
}

// Spec: electric R16
#[test]
fn electrics_sections_are_four_headed_groups_over_a_table_of_three_rows() {
    let panel = placed("Electric", &[]);
    let words: Vec<&str> = panel.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        &words[..5],
        ["Hammer", "Fork", "Damper / Pickup", "Global", "Type"]
    );
    // The table's rows are named at the left of its three rows.
    assert_eq!(&words[5..], ["Stiffness", "Force", "Noise"]);
    // A click on a section's knobs' place shows its page: four regions, each
    // over its own controls.
    let node = &panel.pages[0];
    assert_eq!(node.names, ["Hammer", "Fork", "Damper/Pickup", "Global"]);
    assert_eq!(node.buttons.len(), 4);
    assert!(node.buttons.iter().all(|r| r.width > 0.0 && r.height > 0.0));
    let sizes: Vec<f32> = node.buttons.iter().map(|r| r.x).collect();
    assert!(sizes.windows(2).all(|w| w[0] < w[1]), "left to right");
    // The body is 345 points wide, the display under the sections.
    assert_eq!(panel.size.width, 345.0);
    assert_eq!(panel.size.height, 158.0);
    let display = &panel.cells[0];
    assert!(display.rect.y > 60.0 && display.rect.bottom() <= 158.0 + 1e-3);
}

// Spec: electric R15, R16
#[test]
fn a_new_electric_opens_as_live_shows_it() {
    let read = |name: &str| shown("Electric", name);
    for (name, text) in [
        ("Stiffness", "50 %"),
        ("Noise", "50 %"),
        ("Tine", "50 %"),
        ("Tone", "50 %"),
        ("Pickup Symmetry", "50 %"),
        // The kit reads a gain of 1 as 0.00 dB where Live shows 0.0 dB.
        ("Volume", "0.00 dB"),
        ("Stiffness Vel", "50"),
        ("Stiffness Key", "50"),
        ("Force Amount", "50 %"),
        ("Force Vel", "50"),
        ("Force Key", "50"),
        ("Noise Pitch", "50 %"),
        ("Noise Decay", "50 %"),
        ("Noise Key", "50"),
        ("Tine Color", "50 %"),
        ("Tine Decay", "50 %"),
        ("Tine Key", "50"),
        ("Release", "50 %"),
        ("Tone Decay", "50 %"),
        ("Damper Tone", "50 %"),
        ("Damper Att/Rel", "0 %"),
        ("Damper Level", "50 %"),
        ("Pickup Input", "42 %"),
        ("Pickup Output", "42 %"),
        ("Pickup Out Key", "50"),
        ("Pickup Distance", "50 %"),
        ("Voices", "32"),
        ("Semi", "0 st"),
        ("Detune", "0.00"),
        ("Stretch", "0 %"),
        ("PB Range", "12.00"),
        ("Note PB", "0 st"),
    ] {
        assert_eq!(read(name), text, "{name}");
    }
    // The pickup opens at R, the first of its two types.
    assert_eq!(param("Electric", "Pickup Type").1.default, 0.0);
}

// Spec: external-devices R9
#[test]
fn the_external_instruments_body_is_its_gain_knob_beside_the_section_that_holds_the_rest() {
    // The choosers, Peak and Hardware Latency are the section's, down the
    // left; the body holds Gain alone, a small knob on the row of the Audio
    // From name, so the frame is Live's 129 points: 88 of section and 41 of
    // body.
    assert_eq!(
        controls("External Instrument", &[]),
        of(&[("Gain", Widget::SmallKnob)])
    );
    let panel = placed("External Instrument", &[]);
    assert_eq!(
        (panel.size.width, panel.size.height),
        (41.0, 158.0),
        "the body beside the 88-point section"
    );
    assert_eq!(panel.controls[0].rect.y, 59.1);
    assert_eq!(shown("External Instrument", "Gain"), "0.00 dB");
}

/// The controls of `name`'s panel that show while its parameters read their
/// defaults, with `set` changed: those a "shown while" condition leaves out
/// are not drawn.
fn showing(name: &str, shown: &[(&str, &str)], set: &[(&str, f64)]) -> Vec<(String, Widget)> {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).expect("a device");
    let value = |id: ParamId| {
        set.iter()
            .find(|(n, _)| param(name, n).0 == id)
            .map_or_else(
                || device.description.param(id).expect("a parameter").default,
                |(_, v)| *v,
            )
    };
    placed(name, shown)
        .controls
        .iter()
        .filter(|c| c.shown.is_none_or(|s| s.holds(value(s.param))))
        .map(|c| {
            let drawn = device.description.drawn(&Default::default(), c.param);
            let _ = drawn;
            (
                device.description.param(c.param).unwrap().name.clone(),
                c.widget,
            )
        })
        .collect()
}

// Spec: polysynth R37
#[test]
fn drift_holds_lives_controls_in_lives_order_with_the_lfo_tab_showing() {
    use Widget::{
        Abbreviated, Bare, BareChooser, BareSlider, Chooser, Cycle, Grid, IconToggle, Knob, Number,
        Slider, SmallKnob, Toggle, Unboxed, UnboxedChooser,
    };
    let grid = Grid {
        columns: 2,
        rows: 2,
    };
    let expected: Vec<(&str, Widget)> = vec![
        // The oscillators and the mix.
        ("Osc 1 Wave", UnboxedChooser),
        ("Osc 1 Oct", SmallKnob),
        ("Osc 1 Shape", SmallKnob),
        ("Shape Mod", BareChooser),
        ("Shape Mod Amount", Bare),
        ("Osc 2 Wave", UnboxedChooser),
        ("Osc 2 Oct", SmallKnob),
        ("Osc 2 Detune", SmallKnob),
        ("Pitch Mod 1", UnboxedChooser),
        ("Pitch Mod 2", UnboxedChooser),
        ("Pitch Mod 1 Amount", Unboxed),
        ("Pitch Mod 2 Amount", Unboxed),
        ("Osc 1", Abbreviated(1)),
        ("Osc 2", Abbreviated(1)),
        ("Noise", Abbreviated(1)),
        ("Osc 1 Gain", SmallKnob),
        ("Osc 2 Gain", SmallKnob),
        ("Noise Gain", SmallKnob),
        ("Osc 1 Filter", IconToggle),
        ("Osc 2 Filter", IconToggle),
        ("Noise Filter", IconToggle),
        ("Retrigger", Abbreviated(1)),
        // The filter.
        ("Freq", Knob),
        ("Type", Cycle),
        ("Key", Number),
        ("Res", SmallKnob),
        ("HP", SmallKnob),
        ("Freq Mod 1", BareChooser),
        ("Freq Mod 2", BareChooser),
        ("Freq Mod 1 Amount", BareSlider),
        ("Freq Mod 2 Amount", BareSlider),
        // The envelopes.
        ("Env 1 Attack", SmallKnob),
        ("Env 1 Decay", SmallKnob),
        ("Env 1 Sustain", SmallKnob),
        ("Env 1 Release", SmallKnob),
        ("Env 2 Attack", SmallKnob),
        ("Env 2 Decay", SmallKnob),
        ("Env 2 Sustain", SmallKnob),
        ("Env 2 Release", SmallKnob),
        ("Env 2 Mode", IconToggle),
        // The LFO tab.
        ("LFO Rate", SmallKnob),
        ("LFO Mode", grid),
        ("LFO Shape", UnboxedChooser),
        ("LFO R", Abbreviated(1)),
        ("LFO Amount", SmallKnob),
        ("LFO Mod", BareChooser),
        ("LFO Mod Amount", Bare),
        // The global controls.
        ("Voice Mode", Chooser),
        ("Voices", Chooser),
        ("Drift", Slider),
        ("Legato", Toggle),
        ("Glide Time", Bare),
        ("Volume", SmallKnob),
        ("Vel > Vol", Slider),
        ("Transpose", Number),
        ("Note PB", Toggle),
        ("PB Range", Slider),
    ];
    assert_eq!(showing("Drift", &[], &[]), of(&expected));
}

// Spec: polysynth R37
#[test]
fn drift_shows_the_mod_tab_the_cycling_envelope_and_each_voice_modes_control_in_their_places() {
    use Widget::{Bare, Grid, Slider, SmallKnob, Unboxed, UnboxedChooser};
    // The Mod tab: three slots of a source, its amount and its destination.
    let mod_tab = showing("Drift", &[("Tabs", "Mod")], &[]);
    let first = mod_tab
        .iter()
        .position(|c| c.0 == "Mod 1 Source")
        .expect("the first slot");
    let slots: Vec<(String, Widget)> = mod_tab[first..first + 9].to_vec();
    assert_eq!(
        slots,
        of(&[
            ("Mod 1 Source", UnboxedChooser),
            ("Mod 1 Amount", Unboxed),
            ("Mod 1 Dest", UnboxedChooser),
            ("Mod 2 Source", UnboxedChooser),
            ("Mod 2 Amount", Unboxed),
            ("Mod 2 Dest", UnboxedChooser),
            ("Mod 3 Source", UnboxedChooser),
            ("Mod 3 Amount", Unboxed),
            ("Mod 3 Dest", UnboxedChooser),
        ])
    );
    // Envelope 2 cycling: Rate, Tilt and Hold take the places of Attack,
    // Decay and Sustain, and the time modes that of Release.
    let cycling = showing("Drift", &[], &[("Env 2 Mode", 1.0)]);
    let names: Vec<&str> = cycling.iter().map(|(n, _)| n.as_str()).collect();
    for shown in [
        "Cyc Env Rate",
        "Cyc Env Tilt",
        "Cyc Env Hold",
        "Cyc Env Mode",
    ] {
        assert!(names.contains(&shown), "{shown}");
    }
    for hidden in [
        "Env 2 Attack",
        "Env 2 Decay",
        "Env 2 Sustain",
        "Env 2 Release",
    ] {
        assert!(!names.contains(&hidden), "{hidden}");
    }
    assert!(cycling.contains(&(
        "Cyc Env Mode".to_owned(),
        Grid {
            columns: 2,
            rows: 2
        }
    )));
    // Each voice mode but Poly has its own control between Mode and Voices.
    for (mode, own) in [(1.0, "Thickness"), (2.0, "Spread"), (3.0, "Strength")] {
        let names: Vec<String> = showing("Drift", &[], &[("Voice Mode", mode)])
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert!(names.contains(&own.to_owned()), "{own}");
    }
    assert!(
        !showing("Drift", &[], &[])
            .iter()
            .any(|c| c.0 == "Thickness"),
        "Poly has none"
    );
    let _ = (Bare, Slider, SmallKnob);
}

// Spec: polysynth R36, R37
#[test]
fn a_new_drift_opens_as_live_shows_it() {
    let read = |name: &str| shown("Drift", name);
    for (name, text) in [
        ("Osc 1 Oct", "0"),
        ("Osc 1 Shape", "0.00"),
        ("Shape Mod Amount", "5.0 %"),
        ("Osc 2 Oct", "-1"),
        ("Pitch Mod 1 Amount", "0.0 %"),
        ("Pitch Mod 2 Amount", "0.0 %"),
        ("Osc 1 Gain", "-6.0 dB"),
        ("Osc 2 Gain", "-8.0 dB"),
        ("Noise Gain", "-inf dB"),
        ("Freq", "20.0 kHz"),
        ("Key", "0.00"),
        ("Res", "0.0 %"),
        ("HP", "10.0 Hz"),
        ("Freq Mod 1 Amount", "80 %"),
        ("Freq Mod 2 Amount", "15 %"),
        ("Env 1 Attack", "1.00 ms"),
        ("Env 1 Decay", "600 ms"),
        ("Env 1 Sustain", "70 %"),
        ("Env 1 Release", "600 ms"),
        ("Env 2 Attack", "1.00 ms"),
        ("Env 2 Decay", "600 ms"),
        ("Env 2 Sustain", "20 %"),
        ("Env 2 Release", "600 ms"),
        ("LFO Rate", "0.40 Hz"),
        ("LFO Amount", "100 %"),
        ("LFO Mod Amount", "0.0 %"),
        ("Mod 1 Amount", "80 %"),
        ("Mod 2 Amount", "0.0 %"),
        ("Mod 3 Amount", "0.0 %"),
        ("Voices", "32"),
        ("Drift", "7.2 %"),
        ("Glide Time", "0 ms"),
        ("Volume", "-6.0 dB"),
        ("Vel > Vol", "50 %"),
        ("Transpose", "0 st"),
        ("PB Range", "2 st"),
    ] {
        assert_eq!(read(name), text, "{name}");
    }
    // The choices Live shows first.
    let choice = |name: &str| {
        let (_, p) = param("Drift", name);
        p.format(p.default)
    };
    for (name, text) in [
        ("Osc 1 Wave", "Saw"),
        ("Osc 2 Wave", "Sine"),
        ("Shape Mod", "Slide"),
        ("Pitch Mod 1", "Env 2"),
        ("Pitch Mod 2", "LFO"),
        ("Freq Mod 1", "Env 2"),
        ("Freq Mod 2", "Pressure"),
        ("Type", "I"),
        ("LFO Mode", "Hz"),
        ("LFO Shape", "Sine"),
        ("LFO Mod", "Modwheel"),
        ("Mod 1 Source", "Modwheel"),
        ("Mod 1 Dest", "HP Frequency"),
        ("Mod 2 Source", "Velocity"),
        ("Mod 2 Dest", "None"),
        ("Mod 3 Source", "Pressure"),
        ("Mod 3 Dest", "None"),
        ("Voice Mode", "Poly"),
        ("Note PB", "On"),
        ("Osc 1 Filter", "On"),
        ("Noise", "On"),
    ] {
        assert_eq!(choice(name), text, "{name}");
    }
}

/// Operator's rows down the left and the four rows at the right, as they are
/// with every page: the oscillators D to A, then (after the display's
/// fields) the LFO, the filter, the pitch envelope and the global controls.
fn operator_with(
    fields: &[(&str, Widget)],
    set: &[(&str, f64)],
    page: &str,
) -> (Controls, Controls) {
    use Widget::{Abbreviated, BareChooser, Check, MiniKnob, Segments};
    let mut expected: Vec<(String, Widget)> = Vec::new();
    for op in ["D", "C", "B", "A"] {
        for (what, widget) in [
            ("Coarse", MiniKnob),
            ("Fine", MiniKnob),
            ("Fixed", Check),
            ("Level", MiniKnob),
            ("On", Abbreviated(1)),
        ] {
            expected.push((format!("{op} {what}"), widget));
        }
    }
    expected.extend(of(fields));
    expected.extend(of(&[
        ("LFO On", Check),
        ("LFO Wave", BareChooser),
        ("LFO Range", BareChooser),
        ("LFO Retrigger", Abbreviated(1)),
        ("LFO Rate", MiniKnob),
        ("LFO Amount", MiniKnob),
        ("Filter On", Check),
        ("Filter Type", BareChooser),
        ("Filter Slope", Segments(2)),
        ("Filter Circuit", BareChooser),
        ("Filter Freq", MiniKnob),
        ("Filter Res", MiniKnob),
        ("Pitch Env On", Check),
        ("Pitch Env", MiniKnob),
        ("Spread", MiniKnob),
        ("Transpose", MiniKnob),
        ("Time", MiniKnob),
        ("Tone", MiniKnob),
        ("Volume", MiniKnob),
    ]));
    (showing("Operator", &[("Sections", page)], set), expected)
}

// Spec: operator R38
#[test]
fn operator_holds_lives_controls_in_lives_order_on_each_page() {
    use Widget::{Abbreviated, IconSegments, Unboxed, UnboxedChooser};
    let envelope = |prefix: &str| -> Vec<(String, Widget)> {
        of(&[
            (&format!("{prefix} Attack"), Unboxed),
            (&format!("{prefix} Decay"), Unboxed),
            (&format!("{prefix} Release"), Unboxed),
            (&format!("{prefix} Time < Vel"), Unboxed),
            (&format!("{prefix} Initial"), Unboxed),
            (&format!("{prefix} Peak"), Unboxed),
            (&format!("{prefix} Sustain"), Unboxed),
            (&format!("{prefix} Loop"), UnboxedChooser),
        ])
    };
    let join = |lists: &[Vec<(String, Widget)>]| -> Vec<(&str, Widget)> {
        // Leak the names for the borrowed form `operator_with` takes: a test.
        lists
            .iter()
            .flatten()
            .map(|(n, w)| (&*Box::leak(n.clone().into_boxed_str()), *w))
            .collect()
    };
    // Osc A's page.
    let a = join(&[
        envelope("A"),
        of(&[
            ("A Level < Vel", Unboxed),
            ("A Level < Key", Unboxed),
            ("A Wave", UnboxedChooser),
            ("A Feedback", Unboxed),
            ("A Repeat On", UnboxedChooser),
            ("A Retrigger", Abbreviated(1)),
            ("A Phase", Unboxed),
            ("A Osc < Vel", Unboxed),
            ("A Q", Abbreviated(1)),
        ]),
    ]);
    let (got, want) = operator_with(&a, &[], "Osc A");
    assert_eq!(got, want);
    // The LFO's: the envelope, its End, and the destinations at the right.
    let lfo = join(&[
        envelope("LFO"),
        of(&[
            ("LFO End", Unboxed),
            ("LFO Osc A", Abbreviated(1)),
            ("LFO Osc B", Abbreviated(1)),
            ("LFO Osc C", Abbreviated(1)),
            ("LFO Osc D", Abbreviated(1)),
            ("LFO Filter", Abbreviated(3)),
            ("LFO Amount A", Unboxed),
            ("LFO Amount B", Unboxed),
            ("LFO Dest B", UnboxedChooser),
            ("LFO Rate < Key", Unboxed),
            ("LFO Amt < Vel", Unboxed),
        ]),
    ]);
    let (got, want) = operator_with(&lfo, &[], "LFO");
    assert_eq!(got, want);
    // The global page: the eleven algorithms, the table, and the foot's.
    let mut global: Vec<(&str, Widget)> = vec![("Algorithm", IconSegments(11))];
    for (source, widget_b) in [
        ("Velocity", true),
        ("Key", false),
        ("Aftertouch", true),
        ("Pitch Bend", false),
        ("Mod Wheel", true),
    ] {
        let dest = Box::leak(format!("{source} Dest A").into_boxed_str());
        let amount = Box::leak(format!("{source} Amount A").into_boxed_str());
        global.extend([(&*dest, UnboxedChooser), (&*amount, Unboxed)]);
        if widget_b {
            let dest = Box::leak(format!("{source} Dest B").into_boxed_str());
            let amount = Box::leak(format!("{source} Amount B").into_boxed_str());
            global.extend([(&*dest, UnboxedChooser), (&*amount, Unboxed)]);
        } else {
            global.push((
                if source == "Key" {
                    "Time < Key"
                } else {
                    "PB Range"
                },
                Unboxed,
            ));
        }
    }
    global.extend([
        ("Voices", UnboxedChooser),
        ("Retrigger", Abbreviated(1)),
        ("Interpolation", Widget::Check),
        ("Antialias", Widget::Check),
        ("Pan", Unboxed),
        ("Pan < Key", Unboxed),
        ("Pan < Random", Unboxed),
    ]);
    // The rows' own controls come first, then the table, then the rest.
    let (got, want) = operator_with(&global, &[], "Global");
    assert_eq!(got, want);
}

// Spec: operator R38
#[test]
fn operators_oscillators_fixed_puts_freq_and_multi_in_the_places_of_coarse_and_fine() {
    // Coarse and Fine are the controls placed: Fixed turns them into Freq and
    // Multi, so there is no place of their own for those.
    let all = showing("Operator", &[], &[]);
    let names: Vec<&str> = all.iter().map(|c| c.0.as_str()).collect();
    for op in ["A", "B", "C", "D"] {
        assert!(names.contains(&format!("{op} Coarse").as_str()));
        assert!(!names.contains(&format!("{op} Freq").as_str()));
        assert!(!names.contains(&format!("{op} Multi").as_str()));
    }
    // Key and Pitch Bend's B are Time and Pitch, parameters of their own: the
    // matrix has no Destination B for them.
    for gone in [
        "Key Dest B",
        "Key Amount B",
        "Pitch Bend Dest B",
        "Pitch Bend Amount B",
    ] {
        let types = device_types();
        let operator = types.iter().find(|t| t.name == "Operator").unwrap();
        assert!(
            !operator.description.params.iter().any(|p| p.name == gone),
            "{gone}"
        );
    }
}

// Spec: operator R38
#[test]
fn a_new_operator_opens_as_live_shows_it() {
    let read = |name: &str| shown("Operator", name);
    for (name, text) in [
        ("A Coarse", "1"),
        ("A Fine", "0"),
        ("A Level", "0.00 dB"),
        ("D Level", "-inf dB"),
        // The kit reads a time of 0 as 0 ms where Live shows 0.00 ms.
        ("A Attack", "0 ms"),
        ("A Decay", "1.00 s"),
        ("A Release", "400 ms"),
        ("A Time < Vel", "0 %"),
        ("A Initial", "-inf dB"),
        ("A Peak", "0.00 dB"),
        ("A Sustain", "0.00 dB"),
        ("A Level < Vel", "50 %"),
        ("A Level < Key", "0 %"),
        ("A Feedback", "0 %"),
        ("A Phase", "0 %"),
        ("A Osc < Vel", "0"),
        ("D Decay", "400 ms"),
        ("D Sustain", "-24.0 dB"),
        ("LFO Rate", "100.00"),
        ("LFO Amount", "10 %"),
        ("Filter Freq", "12.0 kHz"),
        ("Filter Res", "28 %"),
        ("Pitch Env", "100 %"),
        ("Spread", "0 %"),
        ("Transpose", "0 st"),
        ("Time", "0 %"),
        ("Tone", "70 %"),
        ("Volume", "-18.0 dB"),
        ("Voices", "6"),
        ("Pan < Key", "0"),
        ("Pan < Random", "0"),
        ("Glide Time", "50.0 ms"),
        ("LFO Rate < Key", "0.0 %"),
        ("Filter Initial", "0.0 %"),
        ("Filter Peak", "100 %"),
        ("Filter Decay", "600 ms"),
        ("Pitch Decay", "400 ms"),
        ("PB Range", "5 st"),
    ] {
        assert_eq!(read(name), text, "{name}");
    }
    // The switches and the choices.
    for (name, text) in [
        ("A On", "On"),
        ("D On", "On"),
        ("LFO On", "Off"),
        ("Filter On", "On"),
        ("Pitch Env On", "Off"),
        ("Filter Type", "Low-pass"),
        ("Filter Slope", "24"),
        ("Filter Circuit", "Clean"),
        ("LFO Wave", "Sine"),
        ("LFO Range", "L"),
        ("A Wave", "Sine"),
        ("A Loop", "None"),
        ("A Retrigger", "On"),
        ("A Q", "On"),
        ("Shaper Type", "Off"),
    ] {
        let (_, p) = param("Operator", name);
        assert_eq!(p.format(p.default), text, "{name}");
    }
}

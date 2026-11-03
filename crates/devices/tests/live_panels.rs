//! Every device that Live has a panel for is laid out as Live draws it, not
//! as the generic grid of all its parameters (the device framework spec,
//! R59). A device is added to [`STILL_GENERIC`] only until its own layout
//! lands, and the list is emptied: a device on it that has a layout of its own
//! fails the test as surely as one off it that hasn't.

use std::collections::BTreeSet;

use bl_devices::device_types;

/// The devices Live's Device View draws a panel for, under their names there:
/// a device Beatlayer hasn't built yet isn't a device type, so it has nothing
/// to lay out here.
const LIVE_PANELS: &[&str] = &[
    // Modulators.
    "Envelope MIDI",
    "Expression Control",
    "Envelope Follower",
    "LFO",
    "Shaper MIDI",
    "Shaper",
    // MIDI effects.
    "Arpeggiator",
    "CC Control",
    "Chord",
    "MIDI Monitor",
    "MPE Control",
    "Note Echo",
    "Note Length",
    "Pitch",
    "Random",
    "Scale",
    "Velocity",
    // Instruments.
    "Analog",
    "Collision",
    "Drift",
    "Drum Sampler",
    "DS Clang",
    "DS Clap",
    "DS Cymbal",
    "DS FM",
    "DS HH",
    "DS Kick",
    "DS Snare",
    "DS Tom",
    "Electric",
    "External Instrument",
    "Impulse",
    "Meld",
    "Operator",
    "Sampler",
    "Simpler",
    "Tension",
    "Wavetable",
    // Audio effects.
    "Align Delay",
    "Amp",
    "Auto Filter",
    "Auto Pan-Tremolo",
    "Auto Shift",
    "Beat Repeat",
    "Cabinet",
    "Channel EQ",
    "Chorus-Ensemble",
    "Compressor",
    "Corpus",
    "Delay",
    "Drum Buss",
    "Dynamic Tube",
    "Echo",
    "EQ Eight",
    "EQ Three",
    "Erosion",
    "External Audio Effect",
    "Filter Delay",
    "Gate",
    "Glue Compressor",
    "Grain Delay",
    "Hybrid Reverb",
    "Limiter",
    "Looper",
    "Multiband Dynamics",
    "Overdrive",
    "Pedal",
    "Phaser-Flanger",
    "Redux",
    "Resonators",
    "Reverb",
    "Roar",
    "Saturator",
    "Shifter",
    "Spectral Resonator",
    "Spectral Time",
    "Spectrum",
    "Tuner",
    "Utility",
    "Vinyl Distortion",
    "Vocoder",
];

/// The built-in devices of [`LIVE_PANELS`] that still draw the generic grid.
/// Each task that lays devices out as Live does takes its devices off.
const STILL_GENERIC: &[&str] = &["External Instrument"];

// Spec: device-framework R59
#[test]
fn a_device_with_a_live_panel_is_laid_out_as_live_draws_it_unless_listed() {
    let types = device_types();
    let built: BTreeSet<&str> = types.iter().map(|t| t.name.as_str()).collect();
    for name in STILL_GENERIC {
        assert!(
            LIVE_PANELS.contains(name) && built.contains(name),
            "{name} is on STILL_GENERIC, so it must be a device of Live's with a type here"
        );
    }
    let generic: BTreeSet<&str> = LIVE_PANELS
        .iter()
        .copied()
        .filter_map(|name| types.iter().find(|t| t.name == name))
        .filter(|t| t.description.has_generic_layout())
        .map(|t| t.name.as_str())
        .collect();
    let listed: BTreeSet<&str> = STILL_GENERIC.iter().copied().collect();
    assert_eq!(
        generic.difference(&listed).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "these still draw the generic grid: lay them out as Live does"
    );
    assert_eq!(
        listed.difference(&generic).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "these have a layout of their own now: take them off STILL_GENERIC"
    );
}

/// The type of the device Live calls `name`.
fn device(name: &str) -> bl_engine::DeviceType {
    device_types()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no device named {name}"))
}

/// The controls of a device's panel as it draws at its defaults, collapsed,
/// in the order it places them (band by band, left to right, top to bottom),
/// each as the text it shows and its kind: a control another's value puts
/// away isn't there. The device's description must check.
fn panel(name: &str) -> Vec<String> {
    let description = device(name).description;
    description.check().unwrap();
    let layout = description.layout.shown(false);
    let default = |id| description.param(id).map_or(0.0, |p| p.default);
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .filter(|c| {
            let at_value =
                |g: bl_model::device_layout::GreyWhen| default(g.param).round() as i32 == g.value;
            !c.hide.is_some_and(at_value) && c.only.is_none_or(at_value)
        })
        .map(|c| {
            let shown = c
                .caption
                .as_deref()
                .unwrap_or_else(|| description.param(c.param).unwrap().name.as_str());
            let kind = match (c.unnamed, c.titled) {
                (true, _) => "unnamed ",
                (_, true) => "titled ",
                _ => "",
            };
            format!("{shown}: {kind}{:?}", c.widget)
        })
        .collect()
}

/// The default of the parameter `id` of the device Live calls `name`.
fn default_of(name: &str, id: bl_model::ParamId) -> f64 {
    device(name).description.param(id).unwrap().default
}

// Spec: delay R15
#[test]
fn delay_holds_lives_controls_in_lives_order() {
    use bl_devices::delay::*;
    assert_eq!(
        panel("Delay"),
        [
            "Sync: Toggle",
            "L Sync: Grid { columns: 2, rows: 4 }",
            "L Offset: unnamed Number",
            "Link: IconToggle",
            "Sync: Toggle",
            "R Sync: Grid { columns: 2, rows: 4 }",
            "R Offset: unnamed Number",
            "Feedback: SmallKnob",
            "Freeze: IconToggle",
            "Filter: Tick",
            "Freq: Inline",
            "Width: Inline",
            "Delay: Inline",
            "Filter: Inline",
            "Time Mode: Stack(3)",
            "Ping Pong: Toggle",
            "Dry/Wet: SmallKnob",
        ]
    );
    // Spec: delay R15. Live's new Delay: Sync on and the dotted eighth's 3
    // sixteenths each side, the filter on at 8.00, Dry/Wet at 50 %.
    for (id, value) in [
        (SYNC, 1.0),
        (R_SYNC_ON, 1.0),
        (L_SYNC, 3.0),
        (R_SYNC, 3.0),
        (FILTER, 1.0),
        (FREQ, 1000.0),
        (WIDTH, 8.0),
        (FEEDBACK, 50.0),
        (TIME_MODE, 0.0),
        (DRY_WET, 50.0),
        (L_OFFSET, 0.0),
        (LFO_DELAY, 0.0),
        (LFO_FILTER, 0.0),
    ] {
        assert_eq!(default_of("Delay", id), value, "parameter {}", id.0);
    }
}

// Spec: alignment-delay R9
#[test]
fn align_delay_holds_lives_controls_in_lives_order() {
    use bl_devices::alignment_delay::*;
    assert_eq!(
        panel("Align Delay"),
        [
            "Delay Mode: Chooser",
            "Left Time: unnamed Number",
            "Right Time: unnamed Number",
            "Link: IconToggle",
        ]
    );
    // Live's new Align Delay: Time mode, no delay, the sides unlinked.
    for (id, value) in [
        (MODE, 0.0),
        (LINK, 0.0),
        (side_param(0, TIME), 0.0),
        (side_param(1, TIME), 0.0),
    ] {
        assert_eq!(default_of("Align Delay", id), value, "parameter {}", id.0);
    }
    // The Samples and Distance modes' boxes take the Time boxes' places.
    let description = device("Align Delay").description;
    let placed = description.layout.place(description.layout.docked_size());
    let rect = |param| {
        placed
            .controls
            .iter()
            .find(|c| c.param == param)
            .unwrap()
            .rect
    };
    for side in 0..2 {
        for field in [SAMPLES, METRES, FEET] {
            assert_eq!(rect(side_param(side, field)), rect(side_param(side, TIME)));
        }
    }
}

// Spec: beat-repeat R15
#[test]
fn beat_repeat_holds_lives_controls_in_lives_order() {
    use bl_devices::beat_repeat::*;
    assert_eq!(
        panel("Beat Repeat"),
        [
            "Interval: SmallKnob",
            "Offset: SmallKnob",
            "Grid: SmallKnob",
            "Variation: SmallKnob",
            "Repeat: Toggle",
            "No Trpl: Toggle",
            "Variation Mode: unnamed Chooser",
            "Chance: SmallKnob",
            "Gate: SmallKnob",
            "Pitch: SmallKnob",
            "Pitch Decay: SmallKnob",
            "Freq: unnamed Number",
            "Width: unnamed Number",
            "Filter: Toggle",
            "Mode: Segments(3)",
            "Volume: SmallKnob",
            "Decay: SmallKnob",
        ]
    );
    // Live's new Beat Repeat: every 1 Bar, a 1/16 grid, a gate of 7, Mix.
    for (id, value) in [
        (INTERVAL, 5.0),
        (OFFSET, 0.0),
        (GRID, 4.0),
        (VARIATION, 0.0),
        (VARIATION_MODE, 0.0),
        (CHANCE, 100.0),
        (GATE, 7.0),
        (PITCH, 0.0),
        (PITCH_DECAY, 0.0),
        (FILTER, 0.0),
        (FREQ, 1000.0),
        (WIDTH, 4.0),
        (MODE, 0.0),
        (VOLUME, 1.0), // 0 dB
        (DECAY, 0.0),
    ] {
        assert_eq!(default_of("Beat Repeat", id), value, "parameter {}", id.0);
    }
}

// Spec: echo R25
#[test]
fn echo_holds_lives_controls_in_lives_order() {
    use bl_devices::echo::*;
    assert_eq!(
        panel("Echo"),
        [
            "Left: SmallKnob",
            "Sync: Toggle",
            "L Sync Mode: unnamed Chooser",
            "L Offset: unnamed Number",
            "Right: SmallKnob",
            "Sync: Toggle",
            "R Sync Mode: unnamed Chooser",
            "R Offset: unnamed Number",
            "Stereo Link: IconToggle",
            "Input: SmallKnob",
            "D: Toggle",
            "Ø: Toggle",
            "Feedback: SmallKnob",
            "Filter: Tick",
            "HP: Inline",
            "Res: Inline",
            "LP: Inline",
            "Res: Inline",
            "Reverb: SmallKnob",
            "Stereo: SmallKnob",
            "Reverb Location: unnamed Chooser",
            "Decay: Slider",
            "Output: SmallKnob",
            "Channel Mode: Stack(3)",
            "Dry/Wet: SmallKnob",
        ]
    );
    // The time knob follows Sync and Sync Mode: milliseconds, or the
    // division of the mode the line is in.
    let description = device("Echo").description;
    let knobs = |sync: f64, mode: f64| {
        let mut values = bl_model::OrdMap::new();
        values.insert(line_param(0, SYNC), sync);
        values.insert(line_param(0, SYNC_MODE), mode);
        let layout = description
            .layout
            .shown(false)
            .with_pages(&Default::default());
        let placed = layout.place(layout.docked_size());
        let value = |id| description.value(&values, id).unwrap();
        placed
            .controls
            .iter()
            .filter(|c| c.caption.as_deref() == Some("Left"))
            .filter(|c| {
                let at =
                    |g: bl_model::device_layout::GreyWhen| value(g.param).round() as i32 == g.value;
                !c.hide.is_some_and(at) && c.only.is_none_or(at)
            })
            .map(|c| c.param)
            .collect::<Vec<_>>()
    };
    assert_eq!(knobs(0.0, 2.0), [line_param(0, TIME)]);
    for (mode, field) in [
        (0.0, NOTES),
        (1.0, TRIPLET),
        (2.0, DOTTED),
        (3.0, SIXTEENTHS),
    ] {
        assert_eq!(knobs(1.0, mode), [line_param(0, field)], "mode {mode}");
    }
    // Live's new Echo: Sync on in Dotted at 1/8 each side, 50 % Feedback, the
    // filter on between 50 Hz and 5 kHz, a reverb Decay of 50 %, 70 % wet.
    for (id, value) in [
        (line_param(0, SYNC), 1.0),
        (line_param(1, SYNC), 1.0),
        (line_param(0, SYNC_MODE), 2.0),
        (line_param(1, SYNC_MODE), 2.0),
        (line_param(0, DOTTED), 3.0),
        (line_param(1, DOTTED), 3.0),
        (line_param(0, OFFSET), 0.0),
        (INPUT, 1.0), // 0 dB, a gain's value being linear
        (FEEDBACK, 50.0),
        (FILTER, 1.0),
        (HP_FREQ, 50.0),
        (HP_RES, 0.0),
        (LP_FREQ, 5_000.0),
        (LP_RES, 0.0),
        (REVERB, 0.0),
        (REVERB_LOCATION, 1.0),
        (DECAY, 50.0),
        (STEREO, 100.0),
        (OUTPUT, 1.0),
        (CHANNEL_MODE, 0.0),
        (DRY_WET, 70.0),
    ] {
        assert_eq!(default_of("Echo", id), value, "parameter {}", id.0);
    }
    // Decay reads in per cent and sets 0.2 s to 10 s along a logarithmic travel.
    assert_eq!(decay_seconds(0.0), 0.2);
    assert!((decay_seconds(100.0) - 10.0).abs() < 1e-9);
    assert!((decay_percent(decay_seconds(37.0)) - 37.0).abs() < 1e-9);
}

// Spec: filter-delay R10
#[test]
fn filter_delay_holds_lives_controls_in_lives_order() {
    use bl_devices::filter_delay::*;
    let grid = "Grid { columns: 4, rows: 2 }";
    let mut expected = Vec::new();
    for (line, name) in ["L", "L+R", "R"].into_iter().enumerate() {
        let n = line + 1;
        expected.extend([
            format!("{name}: Toggle"),
            "On: Toggle".to_owned(),
            format!("{n} Freq: unnamed Slider"),
            format!("{n} Width: unnamed Slider"),
        ]);
    }
    for line in 1..=3 {
        expected.extend([
            format!("{line} Division: {grid}"),
            "Sync: Toggle".to_owned(),
            format!("{line} Time %: unnamed Number"),
            format!("{line} Feedback: unnamed SmallKnob"),
            format!("{line} Pan: unnamed SmallKnob"),
            format!("{line} Volume: unnamed SmallKnob"),
        ]);
    }
    expected.push("Dry: SmallKnob".to_owned());
    assert_eq!(panel("Filter Delay"), expected);
    // Live's new Filter Delay: the lines at 704, 449 and 918 Hz four octaves
    // wide, 3, 1 and 5 sixteenths, panned left, centre and right, with
    // feedbacks of 21, 46 and 24 %, volumes of 4, -4 and 6 dB and a Dry of -5 dB.
    let db = bl_model::param::db_to_gain;
    for (line, (freq, division, pan, volume, feedback)) in [
        (704.0, 3.0, -50.0, 4.0, 21.0),
        (449.0, 1.0, 0.0, -4.0, 46.0),
        (918.0, 5.0, 50.0, 6.0, 24.0),
    ]
    .into_iter()
    .enumerate()
    {
        let default = |id| default_of("Filter Delay", id);
        assert_eq!(default(line_param(line, ON)), 1.0);
        assert_eq!(default(line_param(line, FILTER)), 1.0);
        assert_eq!(default(line_param(line, FREQ)), freq);
        assert_eq!(default(line_param(line, WIDTH)), 4.0);
        assert_eq!(default(line_param(line, SYNC)), 1.0);
        assert_eq!(default(line_param(line, DIVISION)), division);
        assert_eq!(default(line_param(line, PERCENT)), 0.0);
        assert_eq!(default(line_param(line, PAN)), pan);
        assert!((default(line_param(line, VOLUME)) - db(volume)).abs() < 1e-12);
        assert_eq!(default(feedback_param(line)), feedback);
    }
    assert!((default_of("Filter Delay", DRY) - db(-5.0)).abs() < 1e-12);
}

// Spec: grain-delay R10
#[test]
fn grain_delay_holds_lives_controls_in_lives_order() {
    use bl_devices::grain_delay::*;
    assert_eq!(
        panel("Grain Delay"),
        [
            "Spray: unnamed Slider",
            "Frequency: unnamed Slider",
            "Pitch: unnamed Slider",
            "Random Pitch: unnamed Slider",
            "Feedback: unnamed Slider",
            "Dry/Wet: unnamed Slider",
            "Y Axis: Stack(6)",
            "Division: Grid { columns: 4, rows: 2 }",
            "Sync: Toggle",
            "Time %: unnamed Number",
            "X Axis: Segments(6)",
        ]
    );
    // Live's new Grain Delay: synced at 2 sixteenths, 60 Hz, fully wet, the
    // horizontal axis Pitch and the vertical axis Frequency.
    for (id, value) in [
        (SYNC, 1.0),
        (DIVISION, 2.0),
        (PERCENT, 0.0),
        (SPRAY, 0.0),
        (FREQUENCY, 60.0),
        (PITCH, 0.0),
        (RANDOM_PITCH, 0.0),
        (FEEDBACK, 0.0),
        (DRY_WET, 100.0),
        (X_AXIS, 2.0),
        (Y_AXIS, 1.0),
    ] {
        assert_eq!(default_of("Grain Delay", id), value, "parameter {}", id.0);
    }
    // The axes' buttons read Live's names, and each row leaves out the one
    // axis the other offers.
    let description = device("Grain Delay").description;
    let names = |id| match &description.param(id).unwrap().kind {
        bl_model::ParamKind::Choice { choices } => choices.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        names(X_AXIS),
        [
            "Delay Time",
            "Frequency",
            "Pitch",
            "Rand Pitch",
            "Spray",
            "Feedback",
            "DryWet"
        ]
    );
}

// Spec: reverb R16
#[test]
fn reverb_holds_lives_controls_in_lives_order() {
    use bl_devices::reverb::*;
    assert_eq!(
        panel("Reverb"),
        [
            "Lo Cut: Outlined",
            "Hi Cut: Outlined",
            ": Inline",
            ": Inline",
            "Spin: Toggle",
            "Amount: Stacked",
            "Rate: Stacked",
            "Shape: Spread",
            "High: Toggle",
            ": Inline",
            "Low: Outlined",
            ": Inline",
            "Diffusion: Stacked",
            "Scale: Stacked",
            "Chorus: Toggle",
            "Amount: Stacked",
            "Rate: Stacked",
            "Predelay: SmallKnob",
            "Smooth: Chooser",
            "Size: SmallKnob",
            "Decay: SmallKnob",
            "Freeze: Toggle",
            "Flat: Outlined",
            "Cut: Outlined",
            "Stereo: SmallKnob",
            "Density: Chooser",
            "Reflect: SmallKnob",
            "Diffuse: SmallKnob",
            "Dry/Wet: SmallKnob",
        ]
    );
    // Live's new Reverb: the high cut on, Spin at 0.30 Hz, the tail's high
    // damping on, Diffusion 60 % and Scale 40 %, Chorus at 0.02 Hz, a 2.5 ms
    // predelay, Smooth Fast, a 1.2 s Decay, Freeze's Flat and Cut on, the
    // lowest Density, 0 dB Reflect and Diffuse and 55 % wet.
    let db = bl_model::param::db_to_gain;
    for (id, value) in [
        (LOW_CUT, 0.0),
        (HIGH_CUT, 1.0),
        (SPIN, 0.0),
        (SPIN_RATE, 0.3),
        (HIGH_DAMP, 1.0),
        (LOW_DAMP, 0.0),
        (DIFFUSION, 60.0),
        (SCALE, 40.0),
        (CHORUS, 0.0),
        (CHORUS_RATE, 0.02),
        (PREDELAY, 2.5),
        (SMOOTH, 1.0),
        (DECAY, 1.2),
        (FREEZE, 0.0),
        (FLAT, 1.0),
        (CUT, 1.0),
        (DENSITY, 0.0),
        (REFLECT, db(0.0)),
        (DIFFUSE, db(0.0)),
        (DRY_WET, 55.0),
    ] {
        assert_eq!(default_of("Reverb", id), value, "parameter {}", id.0);
    }
    assert_eq!(DENSITIES, ["Low", "Mid", "High"]);
}

// Spec: chorus-ensemble R10
#[test]
fn chorus_ensemble_holds_lives_controls_in_lives_order() {
    use bl_devices::chorus_ensemble::*;
    assert_eq!(
        panel("Chorus-Ensemble"),
        [
            "Mode: Tabs(3)",
            ": Inline",
            "Width: Stacked",
            "Time: Chooser",
            "Taps: titled Segments(2)",
            "Rate: SmallKnob",
            "Amount: SmallKnob",
            "Feedback: SmallKnob",
            "Ø: Toggle",
            "Output: SmallKnob",
            "Warmth: SmallKnob",
            "Dry/Wet: SmallKnob",
        ]
    );
    // Live's new Chorus-Ensemble: Chorus at 0.90 Hz, 50 %, a 50 Hz high-pass,
    // 100 % wide, 20 ms on two taps, no feedback or warmth and 50 % wet.
    for (id, value) in [
        (MODE, 0.0),
        (RATE, 0.9),
        (AMOUNT, 50.0),
        (FEEDBACK, 0.0),
        (FEEDBACK_INVERT, 0.0),
        (HP_FREQ, 50.0),
        (WIDTH, 100.0),
        (CHORUS_TIME, 3.0),
        (CHORUS_TAPS, 1.0),
        (WARMTH, 0.0),
        (DRY_WET, 50.0),
    ] {
        assert_eq!(
            default_of("Chorus-Ensemble", id),
            value,
            "parameter {}",
            id.0
        );
    }
    assert_eq!(CHORUS_TIMES[3], "20 ms");
    assert_eq!(TAPS, ["1", "2"]);
}

// Spec: phaser-flanger R15
#[test]
fn phaser_flanger_holds_lives_controls_in_lives_order() {
    use bl_devices::phaser_flanger::*;
    assert_eq!(
        panel("Phaser-Flanger"),
        [
            "Mode: Tabs(3)",
            "Notches: Stacked",
            "Center: Stacked",
            "Spread: Stacked",
            "Blend: Stacked",
            "Rate: SmallKnob",
            "Amount: SmallKnob",
            "Feedback: SmallKnob",
            "Ø: Toggle",
            "Output: SmallKnob",
            "Warmth: SmallKnob",
            "Dry/Wet: SmallKnob",
        ]
    );
    // Rate is one knob that reads Hz, or the division while its sync
    // button is on, never both.
    let description = device("Phaser-Flanger").description;
    assert!(
        description
            .alternates
            .iter()
            .any(|a| { (a.switch, a.off, a.on) == (LFO_MODE, LFO_HZ, LFO_DIVISION) })
    );
    let placed = description.layout.place(description.layout.docked_size());
    let rate = placed.controls.iter().find(|c| c.param == LFO_HZ).unwrap();
    assert_eq!(
        rate.unit.map(|u| u.units),
        Some([
            bl_model::device_layout::Unit::Hz,
            bl_model::device_layout::Unit::Sync
        ])
    );
    assert!(placed.controls.iter().all(|c| c.param != LFO_DIVISION));
    // Live's new Phaser-Flanger: a Phaser of 2 notches at 400 Hz, a 35 %
    // spread and no blend, 0.15 Hz, 75 %, no feedback, 100 % wet.
    for (id, value) in [
        (MODE, 0.0),
        (NOTCHES, 2.0),
        (CENTER, 400.0),
        (SPREAD, 35.0),
        (BLEND, 0.0),
        (LFO_MODE, 0.0),
        (LFO_HZ, 0.15),
        (AMOUNT, 75.0),
        (FEEDBACK, 0.0),
        (FEEDBACK_INVERT, 0.0),
        (WARMTH, 0.0),
        (DRY_WET, 100.0),
    ] {
        assert_eq!(
            default_of("Phaser-Flanger", id),
            value,
            "parameter {}",
            id.0
        );
    }
}

// Spec: auto-pan-tremolo R17
#[test]
fn auto_pan_tremolo_holds_lives_controls_in_lives_order() {
    use bl_devices::auto_pan_tremolo::*;
    assert_eq!(
        panel("Auto Pan-Tremolo"),
        [
            "Mode: Tabs(2)",
            "Waveform: unnamed Chooser",
            "Time Mode: unnamed Chooser",
            "Stereo Offset: unnamed Chooser",
            ": Inline",
            "Invert: IconToggle",
            ": Inline",
            "Amount: SmallKnob",
            "Rate: SmallKnob",
            "Morph: SmallKnob",
            "Attack Time: unnamed Number",
            "Dynamic FM: unnamed Number",
        ]
    );
    // Live's new Auto Pan-Tremolo: Panning with a sine at 1.00 Hz, 180
    // degrees of phase, no offset, 50 % Amount, no Morph, attack or
    // modulation.
    for (id, value) in [
        (MODE, 0.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (STEREO_MODE, 0.0),
        (PHASE, 180.0),
        (PHASE_OFFSET, 0.0),
        (INVERT, 0.0),
        (AMOUNT, 50.0),
        (SHAPE, 0.0),
        (ATTACK_TIME, 0.0),
        (DYNAMIC_FM, 0.0),
    ] {
        assert_eq!(
            default_of("Auto Pan-Tremolo", id),
            value,
            "parameter {}",
            id.0
        );
    }
}

// Spec: auto-filter R17
#[test]
fn auto_filter_holds_lives_controls_in_lives_order() {
    use bl_devices::auto_filter::*;
    assert_eq!(
        panel("Auto Filter"),
        [
            "Freq: Knob",
            "Type: unnamed Chooser",
            "Slope: Segments(2)",
            "Res: SmallKnob",
            "LFO Stereo: unnamed Chooser",
            "Ø: Inline",
            ": Inline",
            "LFO Quantize: unnamed Chooser",
            ": Inline",
            "Amount: SmallKnob",
            "Rate: SmallKnob",
            "LFO Wave: unnamed Chooser",
            "LFO Morph: unnamed Slider",
            "LFO Mode: unnamed Chooser",
            "Envelope: SmallKnob",
            "Drive: SmallKnob",
            "Circuit: unnamed Chooser",
            "Clip: Toggle",
            "Output: unnamed Slider",
            "Dry/Wet: SmallKnob",
            // Beatlayer's own band: the other types' controls, the
            // envelope's and the sidechain filter's.
            "Morph: Knob",
            "Control: Knob",
            "Pitch: Knob",
            "Formant: Knob",
            "Env Attack: Knob",
            "Env Release: Knob",
            "Env Hold: Toggle",
            "Env S&H: Toggle",
            "Env S&H Rate: Chooser",
            "SC Filter: Toggle",
            "SC Type: Chooser",
            "SC Freq: Knob",
            "SC Q: Knob",
            "SC Shelf Gain: Knob",
            "SC Peak Gain: Knob",
        ]
    );
    // Live's new Auto Filter: a low-pass at 10.0 kHz, 24 dB, no resonance, the
    // LFO's Phase at 4 degrees with no amount, an SVF circuit.
    for (id, value) in [
        (TYPE, 0.0),
        (SLOPE, 2.0),
        (FREQ, 10_000.0),
        (RES, 0.0),
        (CIRCUIT, 0.0),
        (LFO_AMOUNT, 0.0),
        (LFO_HZ, 1.0),
        (LFO_MODE, 0.0),
        (LFO_MORPH, 0.0),
        (LFO_STEREO, 0.0),
        (LFO_PHASE, 4.0),
        (LFO_OFFSET, 0.0),
        (LFO_QUANTIZE, 0.0),
        (LFO_STEPS, 8.0),
        (ENV_AMOUNT, 0.0),
        (DRIVE, 0.0),
        (CLIP, 0.0),
        (DRY_WET, 100.0),
    ] {
        assert_eq!(default_of("Auto Filter", id), value, "parameter {}", id.0);
    }
    assert_eq!(RATE_MODES[0], "Rate");
}

// Spec: looper R29
#[test]
fn looper_holds_lives_parameter_controls_in_lives_order() {
    use bl_devices::looper::*;
    assert_eq!(
        panel("Looper"),
        [
            "Transport: IconSegments(4)",
            "Quantization: Chooser",
            "Record Length: unnamed Chooser",
            "Play/Overdub: IconToggle",
            "Song Control: Chooser",
            "Tempo Control: Chooser",
            "Speed: SmallKnob",
            "Reverse: Toggle",
            "Feedback: SmallKnob",
            "Input → Output: Chooser",
        ]
    );
    // Live's new Looper: stopped, recording for as many bars as it takes and
    // then overdubbing, starting the song and setting its tempo, with the
    // input always heard and a Feedback of 100 %.
    for (id, value) in [
        (TRANSPORT, 3.0),
        (RECORD_LENGTH, 0.0),
        (AFTER_RECORD, 1.0),
        (SONG_CONTROL, 1.0),
        (TEMPO_CONTROL, 2.0),
        (REVERSE, 0.0),
        (FEEDBACK, 100.0),
        (INPUT_OUTPUT, 0.0),
    ] {
        assert_eq!(default_of("Looper", id), value, "parameter {}", id.0);
    }
}

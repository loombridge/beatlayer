//! The modulators' panels hold the controls Live's do, in Live's order and
//! kinds, and none else, with Live's defaults (layout rows L-DEV-016 and
//! L-DEV-021).

use bl_devices::device_types;
use bl_model::device_layout::{DeviceLayout, PageChoice, Widget};
use bl_model::modulator::{MULTIMAP_HIDDEN, MULTIMAP_NODE, MULTIMAP_SHOWN};

/// The type of the device named `name`.
fn type_of(name: &str) -> bl_engine::DeviceType {
    device_types().into_iter().find(|t| t.name == name).unwrap()
}

/// The controls of `layout` as placed, in order: the parameter's name and the
/// widget that draws it.
fn controls_of(name: &str, layout: &DeviceLayout) -> Vec<(String, Widget)> {
    let device = type_of(name);
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| {
            (
                device.description.param(c.param).unwrap().name.clone(),
                c.widget,
            )
        })
        .collect()
}

/// The controls of the panel of the device type named `name`, as its first
/// page lays them out.
fn panel(name: &str) -> Vec<(String, Widget)> {
    controls_of(name, &type_of(name).description.layout)
}

fn default_of(name: &str, param: &str) -> f64 {
    type_of(name)
        .description
        .params
        .iter()
        .find(|p| p.name == param)
        .unwrap_or_else(|| panic!("{name} has no {param}"))
        .default
}

fn expect(name: &str, controls: &[(&str, Widget)]) {
    let want: Vec<(String, Widget)> = controls
        .iter()
        .map(|(n, w)| ((*n).to_owned(), *w))
        .collect();
    assert_eq!(panel(name), want, "{name}'s panel");
}

/// The mapping row along the top of a display cell: Mod, Polarity, and the
/// first slot's Amount, or its Min and Max.
const ROW: [(&str, Widget); 5] = [
    ("Mod", Widget::Cycle),
    ("Polarity", Widget::IconToggle),
    ("Amount 1", Widget::Bare),
    ("Min 1", Widget::Bare),
    ("Max 1", Widget::Bare),
];

fn with_row<'a>(rest: &[(&'a str, Widget)]) -> Vec<(&'a str, Widget)> {
    ROW.iter().copied().chain(rest.iter().copied()).collect()
}

#[test]
fn lfos_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "LFO",
        &with_row(&[
            ("Waveform", Chooser),
            ("Shape", Number),
            ("Steps", Number),
            ("Jitter", Number),
            ("Smooth", Number),
            ("Rate", SmallKnob),
            ("x10", Checkbox),
            ("Depth", SmallKnob),
            ("Offset", SmallKnob),
            ("Phase", Number),
            ("Hold", Toggle),
            ("R", Round),
        ]),
    );
}

#[test]
fn shapers_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Shaper",
        &with_row(&[
            ("Grid", Inline),
            ("Snap", Toggle),
            ("Jitter", Inline),
            ("Smooth", Inline),
            ("R", Round),
            ("Phase", Bare),
            ("Offset", Inline),
            ("T", Round),
            ("Trigger Mode", Segments(3)),
            ("Rate", SmallKnob),
            ("Manual", SmallKnob),
            ("Depth", SmallKnob),
        ]),
    );
}

#[test]
fn shaper_midis_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Shaper MIDI",
        &with_row(&[
            ("Grid", Inline),
            ("Snap", Toggle),
            ("T", Round),
            ("Jitter", Inline),
            ("Smooth", Inline),
            ("Velocity", Number),
            ("Offset", Number),
            ("Loop", Toggle),
            ("Rate", SmallKnob),
            ("Depth", SmallKnob),
            ("Echo", Number),
            ("Time", Number),
        ]),
    );
}

#[test]
fn envelope_followers_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Envelope Follower",
        &with_row(&[
            ("Gain", SmallKnob),
            ("Rise", SmallKnob),
            ("Fall", SmallKnob),
            ("Delay", SmallKnob),
        ]),
    );
}

#[test]
fn envelope_midis_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Envelope MIDI",
        &with_row(&[
            ("Loop Mode", BareChooser),
            ("Sync Rate", BareChooser),
            ("Env Echo Time", Bare),
            ("Env Feedback", Bare),
            ("Global Time", Bare),
            ("Amount", SmallKnob),
            ("Attack", SmallKnob),
            ("Decay", SmallKnob),
            ("Sustain", SmallKnob),
            ("Release", SmallKnob),
            ("Velocity", Toggle),
            ("Attack Slope", Bare),
            ("Decay Slope", Bare),
            ("Sustain Mode", Abbreviated(7)),
            ("Release Slope", Bare),
        ]),
    );
}

#[test]
fn expression_controls_first_tab_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Expression Control",
        &[
            ("Source 1", Chooser),
            ("Amount 1", Map(0)),
            ("Polarity", Cycle),
            ("Amount 1", Bare),
            ("Min 1", Bare),
            ("Max 1", Bare),
            ("Curve A 1", SmallKnob),
            ("Curve Link 1", Round),
            ("Curve B 1", SmallKnob),
            ("Min 1", Inline),
            ("Max 1", Inline),
            ("Curve 1", IconSegments(2)),
            ("Curve X 1", Inline),
            ("Curve Y 1", Inline),
            ("Mod", Cycle),
            ("Increment Steps 1", Bare),
            ("Random Amount 1", Bare),
            ("Smoothing 1", Cycle),
            ("Rise 1", Bare),
            ("Fall 1", Bare),
        ],
    );
}

#[test]
fn expression_controls_tabs_are_live_s_five_in_a_column() {
    let layout = type_of("Expression Control").description.layout;
    let nodes = layout.pages();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].style, bl_model::device_layout::PageStyle::Rail);
    let names: Vec<&str> = nodes[0].pages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Vel", "Press", "Slide", "Rand", "Incr"]);
    // Each tab is its own page, holding its own parameters.
    let mut seen = std::collections::BTreeSet::new();
    for (n, name) in names.iter().enumerate() {
        let choice = PageChoice::from([("Tabs".to_owned(), (*name).to_owned())]);
        let shown = layout.with_pages(&choice);
        let source = bl_devices::play_mod::param(n, bl_devices::play_mod::Field::Source);
        let placed = shown.place(shown.docked_size());
        assert!(
            placed.controls.iter().any(|c| c.param == source),
            "{name} shows its own Source"
        );
        assert!(seen.insert(source));
    }
}

#[test]
fn every_modulators_multimap_page_lists_the_other_seven_slots() {
    for name in [
        "LFO",
        "Shaper",
        "Shaper MIDI",
        "Envelope Follower",
        "Envelope MIDI",
    ] {
        let layout = type_of(name).description.layout;
        let choice = PageChoice::from([(MULTIMAP_NODE.to_owned(), MULTIMAP_SHOWN.to_owned())]);
        let shown = layout.with_pages(&choice);
        let amounts: Vec<String> = controls_of(name, &shown)
            .into_iter()
            .filter(|(n, w)| n.starts_with("Amount ") && *w == Widget::Bare)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            amounts,
            (2..=8).map(|n| format!("Amount {n}")).collect::<Vec<_>>(),
            "{name}'s list"
        );
        let hidden = PageChoice::from([(MULTIMAP_NODE.to_owned(), MULTIMAP_HIDDEN.to_owned())]);
        assert_eq!(
            controls_of(name, &layout.with_pages(&hidden)),
            panel(name),
            "{name}'s own panel is the page a new device shows"
        );
    }
}

#[test]
fn lfos_defaults_are_lives() {
    let d = |p: &str| default_of("LFO", p);
    assert_eq!(d("Rate"), 1.0, "Hz");
    assert_eq!(d("Time Mode"), 0.0, "Hz");
    assert_eq!(d("Depth"), 100.0);
    assert_eq!(d("Offset"), 0.0);
    assert_eq!(d("Phase"), 0.0);
    assert_eq!(d("Shape"), 0.0);
    assert_eq!(d("Steps"), 0.0, "Off");
    assert_eq!(d("Jitter"), 0.0);
    assert_eq!(d("Smooth"), 0.0);
    assert_eq!(d("Waveform"), 0.0, "Sine");
    assert_eq!(d("Amount 1"), 50.0);
    assert_eq!(d("Mod"), 0.0, "Mod");
    assert_eq!(d("Polarity"), 0.0, "Bipolar");
    assert_eq!(d("Min 1"), 0.0);
    assert_eq!(d("Max 1"), 100.0);
}

#[test]
fn shapers_defaults_are_lives() {
    for name in ["Shaper", "Shaper MIDI"] {
        let d = |p: &str| default_of(name, p);
        assert_eq!(d("Time Mode"), 1.0, "{name}: the rate starts in sync");
        assert_eq!(d("Beat Rate"), 3.0, "{name}: 1/4");
        assert_eq!(d("Depth"), 100.0);
        assert_eq!(d("Jitter"), 0.0);
        assert_eq!(d("Smooth"), 0.0);
        assert_eq!(d("Offset"), 0.0);
        assert_eq!(d("Grid"), 1.0, "{name}: 4");
        assert_eq!(d("Snap"), 1.0, "{name}: lit");
        assert_eq!(d("Amount 1"), 50.0);
    }
    assert_eq!(default_of("Shaper", "Phase"), 0.0);
    assert_eq!(default_of("Shaper", "Trigger Mode"), 0.0, "Loop");
    assert_eq!(default_of("Shaper MIDI", "Velocity"), 0.0);
    assert_eq!(default_of("Shaper MIDI", "Loop"), 0.0);
    assert_eq!(default_of("Shaper MIDI", "Echo"), 0.0);
    assert_eq!(default_of("Shaper MIDI", "Time"), 100.0, "ms");
}

#[test]
fn envelope_followers_defaults_are_lives() {
    let d = |p: &str| default_of("Envelope Follower", p);
    assert_eq!(d("Gain"), 1.0, "0.0 dB");
    assert_eq!(d("Rise"), 0.0, "per cent");
    assert_eq!(d("Fall"), 0.0, "per cent");
    assert_eq!(d("Delay"), 0.0, "ms");
    assert_eq!(d("Delay Mode"), 0.0, "ms");
    assert_eq!(d("Amount 1"), 50.0);
    // Rise and Fall read in per cent, Delay in ms up to 500, as Live's do.
    let device = type_of("Envelope Follower");
    let read = |p: &str, v: f64| {
        let param = device
            .description
            .params
            .iter()
            .find(|q| q.name == p)
            .unwrap();
        (param.format(v), param.max())
    };
    assert_eq!(read("Rise", 50.0), ("50.00 %".to_owned(), 100.0));
    assert_eq!(read("Fall", 100.0), ("100.00 %".to_owned(), 100.0));
    assert_eq!(read("Delay", 500.0), ("500.00 ms".to_owned(), 500.0));
    assert!(device.description.params.iter().all(|p| p.name != "Depth"));
}

#[test]
fn envelope_midis_defaults_are_lives() {
    let d = |p: &str| default_of("Envelope MIDI", p);
    assert_eq!(d("Amount"), 100.0);
    assert_eq!(d("Attack"), 5.0, "ms");
    assert_eq!(d("Decay"), 333.0, "ms");
    assert_eq!(d("Release"), 333.0, "ms");
    assert!(
        (d("Sustain") - 10f64.powf(-6.0 / 20.0)).abs() < 1e-9,
        "-6.0 dB"
    );
    assert_eq!(d("Velocity"), 1.0, "on");
    assert_eq!(d("Sustain Mode"), 1.0, "on");
    assert_eq!(d("Attack Slope"), 0.0);
    assert_eq!(d("Decay Slope"), 0.0);
    assert_eq!(d("Release Slope"), 0.0);
    assert_eq!(d("Global Time"), 1.0, "Time × 1.00");
    assert_eq!(d("Loop Mode"), 0.0, "Free");
    assert_eq!(d("Amount 1"), 50.0);
}

#[test]
fn expression_controls_defaults_are_lives() {
    let d = |p: &str| default_of("Expression Control", p);
    assert_eq!(d("Polarity"), 1.0, "Unipolar");
    assert_eq!(d("Mod"), 0.0, "Modulate");
    assert_eq!(d("Amount 1"), 50.0);
    assert_eq!(d("Min 1"), 0.0);
    // Each tab follows its own source to begin with.
    let sources: Vec<f64> = (1..=5).map(|n| d(&format!("Source {n}"))).collect();
    assert_eq!(
        sources,
        [0.0, 3.0, 8.0, 6.0, 7.0],
        "Velocity, Pressure, Slide, Random, Increment"
    );
    assert_eq!(d("Increment Steps 1"), 8.0);
    assert_eq!(d("Random Amount 1"), 50.0);
    assert_eq!(d("Smoothing 1"), 1.0, "Log");
    assert_eq!(d("Rise 1"), 0.0);
    assert_eq!(d("Fall 1"), 0.0);
    assert_eq!(d("Curve 1"), 0.0, "two breakpoints");
    assert_eq!(d("Curve A 1"), 0.0);
    assert_eq!(d("Curve X 1"), 64.0);
    assert_eq!(d("Curve Y 1"), 64.0);
    // Min and Max read 0 and 127.
    let device = type_of("Expression Control");
    let tab_min = device
        .description
        .param(bl_devices::play_mod::param(
            0,
            bl_devices::play_mod::Field::Min,
        ))
        .unwrap();
    let tab_max = device
        .description
        .param(bl_devices::play_mod::param(
            0,
            bl_devices::play_mod::Field::Max,
        ))
        .unwrap();
    assert_eq!((tab_min.default, tab_max.default), (0.0, 127.0));
}

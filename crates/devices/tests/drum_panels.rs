//! The drum instruments' panels as Live lays them out (layout L-DEV-047 to
//! L-DEV-049): each holds the controls Live's panel shows, in its order and
//! kinds, and none else, at the sizes measured on Live 12.4.16.

use bl_devices::device_types;
use bl_model::device_layout::{Placed, Widget};

/// The placed panel of the device named `name` at its docked size.
fn placed(name: &str) -> Placed {
    let types = device_types();
    let device = types
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} is a device"));
    let layout = &device.description.layout;
    layout.place(layout.docked_size())
}

/// The panel's controls in the layout's order, as name and kind.
fn controls(name: &str) -> Vec<(String, Widget)> {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).expect("a device");
    placed(name)
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

// Spec: drum-synths R6
#[test]
fn ds_clap_holds_lives_controls_in_lives_order() {
    assert_eq!(
        controls("DS Clap"),
        of(&[
            ("Sloppy", Widget::Large),
            ("Tail", Widget::Large),
            ("Spread", Widget::Framed),
            ("Tone", Widget::Framed),
            ("Tune", Widget::SmallKnob),
            ("Decay", Widget::SmallKnob),
            ("Volume", Widget::SmallKnob),
        ])
    );
    let panel = placed("DS Clap");
    assert_eq!(
        panel
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        ["Spread/Tone"]
    );
}

// Spec: drum-synths R6
#[test]
fn ds_kick_holds_lives_controls_in_lives_order() {
    assert_eq!(
        controls("DS Kick"),
        of(&[
            ("Pitch", Widget::Large),
            ("Drive", Widget::Framed),
            ("OT", Widget::Framed),
            ("Attack", Widget::Framed),
            ("Click", Widget::Toggle),
            ("Ø", Widget::Toggle),
            ("Decay", Widget::SmallKnob),
            ("Env", Widget::SmallKnob),
            ("Volume", Widget::SmallKnob),
        ])
    );
    let panel = placed("DS Kick");
    assert_eq!(
        panel
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        ["Drive/OT", "Attack"]
    );
}

// Spec: drum-synths R6
#[test]
fn ds_snare_holds_lives_controls_in_lives_order() {
    assert_eq!(
        controls("DS Snare"),
        of(&[
            ("Color", Widget::Large),
            ("Tone", Widget::Large),
            ("Filter", Widget::Stack(3)),
            ("Decay", Widget::SmallKnob),
            ("Tune", Widget::SmallKnob),
            ("Volume", Widget::SmallKnob),
        ])
    );
}

// Spec: drum-synths R6
#[test]
fn ds_tom_holds_lives_controls_in_lives_order() {
    assert_eq!(
        controls("DS Tom"),
        of(&[
            ("Pitch", Widget::Large),
            ("Color", Widget::Large),
            ("Tone", Widget::Large),
            ("Bend", Widget::SmallKnob),
            ("Decay", Widget::SmallKnob),
            ("Volume", Widget::SmallKnob),
        ])
    );
}

// Spec: drum-synths R6
#[test]
fn a_drum_synth_is_a_display_with_three_knobs_under_it_at_lives_size() {
    for name in ["DS Clap", "DS Kick", "DS Snare", "DS Tom"] {
        let types = device_types();
        let layout = &types
            .iter()
            .find(|t| t.name == name)
            .unwrap()
            .description
            .layout;
        let size = layout.docked_size();
        // The 145-point frame and the 158-point body of Live's panels.
        assert_eq!((size.width, size.height), (145.0, 158.0), "{name}");
        let panel = placed(name);
        let cell = &panel.cells[0];
        assert_eq!(
            (cell.rect.x, cell.rect.y, cell.rect.width, cell.rect.height),
            (4.0, 4.0, 137.0, 101.0),
            "{name}'s display, 4 points in from the frame"
        );
        let knobs: Vec<_> = panel
            .controls
            .iter()
            .filter(|c| c.widget == Widget::SmallKnob)
            .collect();
        assert_eq!(knobs.len(), 3, "{name}");
        assert!(
            knobs.iter().all(|k| k.rect.y > cell.rect.bottom()),
            "{name}"
        );
    }
}

// Spec: one-shot-drum-sampler R29
#[test]
fn drum_sampler_holds_lives_controls_in_lives_order() {
    // Column by column: the sample's display and the knobs under it, the
    // effect's display and its knobs, the filter and the global column.
    assert_eq!(
        controls("Drum Sampler"),
        of(&[
            ("Start", Widget::Inline),
            ("Length", Widget::Inline),
            ("Gain", Widget::Inline),
            ("Attack", Widget::SmallKnob),
            ("Hold", Widget::SmallKnob),
            ("Decay", Widget::SmallKnob),
            ("Transpose", Widget::Bare),
            ("Detune", Widget::Bare),
            ("Env Mode", Widget::Cycle),
            ("FX", Widget::Toggle),
            ("Effect Type", Widget::BareChooser),
            ("FX1", Widget::SmallKnob),
            ("FX2", Widget::SmallKnob),
            ("Filter", Widget::Toggle),
            (
                "Filter Type",
                Widget::Grid {
                    columns: 2,
                    rows: 2
                }
            ),
            ("Res", Widget::SmallKnob),
            ("Freq", Widget::SmallKnob),
            ("Volume", Widget::SmallKnob),
            ("Pan", Widget::Bare),
            ("Vel > Vol", Widget::Slider),
            ("Mod Source", Widget::Cycle),
            ("Mod Destination", Widget::BareChooser),
            ("Mod Amount", Widget::Bare),
        ])
    );
    let panel = placed("Drum Sampler");
    assert_eq!(
        panel
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        ["Transp", "Detune", "Mode", "Mod"]
    );
    // Two displays, the sample's and the effect's, at Live's sizes.
    assert_eq!(panel.cells.len(), 2);
    let sizes: Vec<_> = panel
        .cells
        .iter()
        .map(|c| (c.rect.width, c.rect.height))
        .collect();
    assert_eq!(sizes, [(236.8, 101.7), (99.9, 101.7)]);
}

// Spec: eight-slot-drum-sampler R22
#[test]
fn impulse_holds_lives_controls_in_lives_order() {
    use Widget::{Abbreviated, Bare, BareChooser, BareSlider, Cycle, Knob, Toggle};
    let slot_one = controls("Impulse");
    let first: Vec<_> = slot_one.iter().take(24).cloned().collect();
    assert_eq!(
        first,
        of(&[
            ("Slot 1 Start", Knob),
            ("Slot 1 Transpose", Knob),
            ("Slot 1 Transpose Velocity", Bare),
            ("Slot 1 Transpose Random", Bare),
            ("Slot 1 Stretch", Knob),
            ("Slot 1 Stretch Velocity", Bare),
            ("Slot 1 Stretch Mode", Cycle),
            ("Slot 1 Drive", Knob),
            ("Slot 1 Saturator", Abbreviated(3)),
            ("Slot 1 Filter Freq", Knob),
            ("Slot 1 Filter", Toggle),
            ("Slot 1 Filter Type", BareChooser),
            ("Slot 1 Filter Res", Knob),
            ("Slot 1 Filter Velocity", Bare),
            ("Slot 1 Filter Random", Bare),
            ("Slot 1 Decay", Knob),
            ("Slot 1 Decay Mode", Cycle),
            ("Slot 1 Pan", Knob),
            ("Slot 1 Pan Velocity", Bare),
            ("Slot 1 Pan Random", Bare),
            ("Slot 1 Volume", Knob),
            ("Slot 1 Volume Velocity", BareSlider),
            ("Slot 1 Mute", Abbreviated(1)),
            ("Slot 1 Solo", Abbreviated(1)),
        ])
    );
    // Then the global knobs; Link is a menu option, not on the panel.
    assert_eq!(
        slot_one[24..],
        of(&[("Volume", Knob), ("Time", Knob), ("Transpose", Knob)])
    );
    let types = device_types();
    let impulse = types.iter().find(|t| t.name == "Impulse").unwrap();
    assert_eq!(impulse.description.menu_options.len(), 1);
}

// Spec: eight-slot-drum-sampler R18
#[test]
fn impulse_is_eight_cells_over_the_selected_slots_columns_at_lives_size() {
    let types = device_types();
    let layout = &types
        .iter()
        .find(|t| t.name == "Impulse")
        .unwrap()
        .description
        .layout;
    let size = layout.docked_size();
    assert_eq!((size.width.round(), size.height), (418.0, 158.0));
    let panel = placed("Impulse");
    assert_eq!(panel.cells.len(), 8);
    for (n, cell) in panel.cells.iter().enumerate() {
        let x = 1.7 + 1.175 + n as f32 * 46.45;
        assert!((cell.rect.x - x).abs() < 0.01, "cell {n}");
        assert!((cell.rect.width - 44.1).abs() < 0.01 && (cell.rect.height - 43.5).abs() < 0.01);
        assert!((cell.rect.y - 2.875).abs() < 0.01);
    }
}

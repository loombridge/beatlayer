//! The dynamics, EQ and utility panels hold the controls Live's do, in
//! Live's order and kinds, and none else, with Live's defaults (layout rows
//! L-DEV-047 to L-DEV-058).

use bl_devices::device_types;
use bl_model::device_layout::Widget;

/// The controls of the panel of the device type named `name`, left to right
/// as the layout places them: the parameter's name and the widget that
/// draws it.
fn panel(name: &str) -> Vec<(String, Widget)> {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).unwrap();
    let layout = &device.description.layout;
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| {
            let param = device.description.param(c.param).unwrap();
            (param.name.clone(), c.widget)
        })
        .collect()
}

/// The default of the parameter called `param` of the device named `name`.
fn default_of(name: &str, param: &str) -> f64 {
    let types = device_types();
    let device = types.iter().find(|t| t.name == name).unwrap();
    device
        .description
        .params
        .iter()
        .find(|p| p.name == param)
        .unwrap()
        .default
}

fn expect(name: &str, controls: &[(&str, Widget)]) {
    let want: Vec<(String, Widget)> = controls
        .iter()
        .map(|(n, w)| ((*n).to_owned(), *w))
        .collect();
    assert_eq!(panel(name), want, "{name}'s panel");
}

#[test]
fn compressors_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Compressor",
        &[
            ("SC Filter", Toggle),
            ("Type", Chooser),
            ("Freq", Knob),
            ("Res", Slider),
            ("Ratio", SmallKnob),
            ("Attack", SmallKnob),
            ("Release", SmallKnob),
            ("Auto", Toggle),
            ("Threshold", Number),
            ("Out", Number),
            ("Knee", Number),
            ("Look", Chooser),
            ("Env", Chooser),
            ("Makeup", Toggle),
            ("Mode", Stack(3)),
            ("Dry/Wet", Knob),
        ],
    );
}

#[test]
fn compressors_defaults_are_lives() {
    let d = |p: &str| default_of("Compressor", p);
    assert_eq!(d("Threshold"), 0.0);
    assert_eq!(d("Ratio"), 4.0);
    assert_eq!(d("Attack"), 1.0);
    assert_eq!(d("Release"), 30.0);
    assert_eq!(d("Knee"), 6.0);
    assert_eq!(d("Mode"), 1.0, "RMS");
    assert_eq!(d("Env"), 1.0, "Log");
    assert_eq!(d("SC Filter"), 1.0);
    assert_eq!(d("Type"), 2.0, "High Pass");
    assert_eq!(d("Freq"), 80.0);
    assert_eq!(d("Res"), 0.71);
    assert_eq!(d("Dry/Wet"), 100.0);
}

#[test]
fn glue_compressors_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Glue Compressor",
        &[
            ("SC EQ", Toggle),
            ("Type", Chooser),
            ("Freq", Knob),
            ("Res", Slider),
            ("Attack", SmallKnob),
            ("Release", SmallKnob),
            ("Auto", Toggle),
            ("Ratio", SmallKnob),
            ("Threshold", Knob),
            ("Makeup", Knob),
            ("Soft Clip", Toggle),
            ("Range", Knob),
            ("Dry/Wet", Knob),
        ],
    );
}

#[test]
fn glue_compressors_defaults_are_lives() {
    let d = |p: &str| default_of("Glue Compressor", p);
    assert_eq!(d("Threshold"), 0.0);
    assert_eq!(d("Ratio"), 4.0);
    assert_eq!(d("Range"), 70.0);
    assert_eq!(d("Makeup"), 1.0, "0 dB, as a gain");
    assert_eq!(d("Dry/Wet"), 100.0);
}

#[test]
fn limiters_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Limiter",
        &[
            ("Gain", Knob),
            ("Maximize", Toggle),
            ("Release", Knob),
            ("Auto", Toggle),
            ("Ceiling", Number),
            ("Link", Slider),
            ("Lookahead", Chooser),
            ("Ceiling Mode", Stack(3)),
            ("Routing Mode", Segments(2)),
        ],
    );
}

#[test]
fn limiters_defaults_are_lives() {
    let d = |p: &str| default_of("Limiter", p);
    assert_eq!(d("Gain"), 1.0, "0 dB, as a gain");
    assert_eq!(d("Ceiling"), -0.3);
    assert_eq!(d("Release"), 100.0);
    assert_eq!(d("Auto"), 0.0);
    assert_eq!(d("Link"), 100.0);
    assert_eq!(d("Ceiling Mode"), 0.0, "Standard");
    assert_eq!(d("Routing Mode"), 0.0, "L/R");
}

#[test]
fn gates_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Gate",
        &[
            ("SC EQ", Toggle),
            ("Type", Chooser),
            ("Freq", Knob),
            ("Res", Slider),
            ("Threshold", Knob),
            ("Return", Knob),
            ("Flip", Toggle),
            ("Lookahead", Chooser),
            ("Attack", SmallKnob),
            ("Hold", SmallKnob),
            ("Release", SmallKnob),
            ("Floor", SmallKnob),
        ],
    );
}

#[test]
fn gates_defaults_are_lives() {
    let d = |p: &str| default_of("Gate", p);
    assert_eq!(d("Threshold"), -12.0);
    assert_eq!(d("Return"), 3.0);
    assert_eq!(d("Attack"), 3.5);
    assert_eq!(d("Hold"), 10.0);
    assert_eq!(d("Release"), 15.0);
    assert!(
        (d("Floor") - 10f64.powf(-40.0 / 20.0)).abs() < 1e-9,
        "-40 dB"
    );
}

#[test]
fn multiband_dynamics_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Multiband Dynamics",
        &[
            ("High", Toggle),
            ("High Active", Toggle),
            ("High Freq", Slider),
            ("High Solo", Toggle),
            ("Mid Active", Toggle),
            ("Mid Solo", Toggle),
            ("Low", Toggle),
            ("Low Active", Toggle),
            ("Low Freq", Slider),
            ("Low Solo", Toggle),
            ("High Input", SmallKnob),
            ("Mid Input", SmallKnob),
            ("Low Input", SmallKnob),
            ("High Below Threshold", Number),
            ("High Below Ratio", Number),
            ("Mid Below Threshold", Number),
            ("Mid Below Ratio", Number),
            ("Low Below Threshold", Number),
            ("Low Below Ratio", Number),
            ("High Below Attack", Number),
            ("High Below Release", Number),
            ("Mid Below Attack", Number),
            ("Mid Below Release", Number),
            ("Low Below Attack", Number),
            ("Low Below Release", Number),
            ("High Above Threshold", Number),
            ("High Above Ratio", Number),
            ("Mid Above Threshold", Number),
            ("Mid Above Ratio", Number),
            ("Low Above Threshold", Number),
            ("Low Above Ratio", Number),
            ("High Above Attack", Number),
            ("High Above Release", Number),
            ("Mid Above Attack", Number),
            ("Mid Above Release", Number),
            ("Low Above Attack", Number),
            ("Low Above Release", Number),
            ("High Output", SmallKnob),
            ("Mid Output", SmallKnob),
            ("Low Output", SmallKnob),
            ("Output", SmallKnob),
            ("Time", SmallKnob),
            ("Amount", SmallKnob),
            ("Soft Knee", Toggle),
            ("Detector", Segments(2)),
        ],
    );
}

#[test]
fn drum_buss_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Drum Buss",
        &[
            ("Drive", Knob),
            ("Type", Stack(3)),
            ("Trim", Slider),
            ("Comp", Toggle),
            ("Crunch", SmallKnob),
            ("Damp", SmallKnob),
            ("Transients", SmallKnob),
            ("Boom", Knob),
            ("Freq", Knob),
            ("Decay", Slider),
            ("Force To Note", Toggle),
            ("Boom Audition", Round),
            ("Output", Marked),
            ("Dry/Wet", Slider),
        ],
    );
}

#[test]
fn channel_eq_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Channel EQ",
        &[
            ("HP 80", IconToggle),
            ("Mid Freq", Number),
            ("Low", SmallKnob),
            ("Mid", SmallKnob),
            ("High", SmallKnob),
            ("Output", SmallKnob),
        ],
    );
}

#[test]
fn eq_three_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "EQ Three",
        &[
            ("Gain Low", Knob),
            ("On Low", Toggle),
            ("Gain Mid", Knob),
            ("On Mid", Toggle),
            ("Gain High", Knob),
            ("On High", Toggle),
            ("FreqLo", SmallKnob),
            ("Slope", Stack(2)),
            ("FreqHi", SmallKnob),
        ],
    );
}

#[test]
fn eq_eight_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "EQ Eight",
        &[
            ("A1 Freq", SmallKnob),
            ("A1 Gain", SmallKnob),
            ("A1 Q", SmallKnob),
            ("A1 Type", Chooser),
            ("A1 On", Toggle),
            ("A2 Type", Chooser),
            ("A2 On", Toggle),
            ("A3 Type", Chooser),
            ("A3 On", Toggle),
            ("A4 Type", Chooser),
            ("A4 On", Toggle),
            ("A5 Type", Chooser),
            ("A5 On", Toggle),
            ("A6 Type", Chooser),
            ("A6 On", Toggle),
            ("A7 Type", Chooser),
            ("A7 On", Toggle),
            ("A8 Type", Chooser),
            ("A8 On", Toggle),
            ("Analyze", Toggle),
            ("Mode", Chooser),
            ("Adaptive Q", Toggle),
            ("Gain", Number),
        ],
    );
}

#[test]
fn utility_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Utility",
        &[
            ("Phase Left", Toggle),
            ("Phase Right", Toggle),
            ("Channel Mode", Chooser),
            ("Width", Knob),
            ("Mono", Toggle),
            ("Bass Mono", Toggle),
            ("Bass Mono Frequency", Slider),
            ("Gain", Knob),
            ("Balance", Knob),
            ("Mute", Toggle),
            ("DC", Toggle),
        ],
    );
}

#[test]
fn spectrum_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Spectrum",
        &[
            ("Block", Chooser),
            ("Channel", Segments(3)),
            ("Refresh", Slider),
            ("Avg", Number),
            ("Graph", Segments(2)),
            ("Max", Toggle),
            ("Scale X", Segments(3)),
            ("Range", Segments(2)),
            ("Range Max", Number),
            ("Range Min", Number),
            ("Clear Max", Toggle),
        ],
    );
}

#[test]
fn tuner_panel_holds_lives_controls_in_lives_order() {
    use Widget::*;
    expect(
        "Tuner",
        &[
            ("Classic Mode", Segments(2)),
            ("Readout", Segments(2)),
            ("Auto", Toggle),
            ("Reference", Number),
        ],
    );
}

#[test]
fn multiband_dynamics_defaults_are_lives() {
    let d = |p: &str| default_of("Multiband Dynamics", p);
    assert_eq!(d("Low Freq"), 120.0);
    assert_eq!(d("High Freq"), 2_500.0);
    assert_eq!(d("Detector"), 1.0, "RMS");
    assert_eq!(d("Soft Knee"), 0.0);
    assert_eq!(d("Time"), 100.0);
    assert_eq!(d("Amount"), 100.0);
    for (band, (attack, release)) in [
        ("High", (5.0, 100.0)),
        ("Mid", (10.0, 200.0)),
        ("Low", (50.0, 300.0)),
    ] {
        assert_eq!(d(&format!("{band} Above Threshold")), -20.0);
        assert_eq!(d(&format!("{band} Below Threshold")), -60.0);
        assert_eq!(d(&format!("{band} Above Ratio")), 1.0);
        assert_eq!(d(&format!("{band} Above Attack")), attack);
        assert_eq!(d(&format!("{band} Above Release")), release);
    }
}

#[test]
fn drum_buss_defaults_are_lives() {
    let d = |p: &str| default_of("Drum Buss", p);
    assert_eq!(d("Drive"), 20.0);
    assert_eq!(d("Type"), 0.0, "Soft");
    assert_eq!(d("Crunch"), 0.0);
    assert_eq!(d("Damp"), 9_200.0, "Hz");
    assert_eq!(d("Transients"), 0.0);
    assert_eq!(d("Boom"), 0.0);
    assert_eq!(d("Freq"), 50.0);
    assert_eq!(d("Decay"), 100.0, "per cent");
    assert_eq!(d("Dry/Wet"), 100.0);
    assert_eq!(d("Trim"), 1.0, "0 dB, as a gain");
}

#[test]
fn channel_eqs_defaults_are_lives() {
    let d = |p: &str| default_of("Channel EQ", p);
    assert_eq!(d("Mid Freq"), 1_500.0);
    for band in ["Low", "Mid", "High"] {
        assert_eq!(d(band), 0.0);
    }
    assert_eq!(d("Output"), 1.0, "0 dB, as a gain");
}

#[test]
fn eq_threes_defaults_are_lives() {
    let d = |p: &str| default_of("EQ Three", p);
    assert_eq!(d("FreqLo"), 250.0);
    assert_eq!(d("FreqHi"), 2_500.0);
    assert_eq!(d("Slope"), 1.0, "48 dB");
    for band in ["Low", "Mid", "High"] {
        assert_eq!(d(&format!("Gain {band}")), 1.0, "0 dB, as a gain");
        assert_eq!(d(&format!("On {band}")), 1.0);
    }
}

#[test]
fn utilitys_defaults_are_lives() {
    let d = |p: &str| default_of("Utility", p);
    assert_eq!(d("Width"), 100.0);
    assert_eq!(d("Gain"), 1.0, "0 dB, as a gain");
    assert_eq!(d("Balance"), 0.0, "centre");
    assert_eq!(d("Bass Mono Frequency"), 120.0);
    for off in [
        "Phase Left",
        "Phase Right",
        "Mono",
        "Bass Mono",
        "Mute",
        "DC",
    ] {
        assert_eq!(d(off), 0.0, "{off}");
    }
}

#[test]
fn spectrums_defaults_are_lives() {
    let d = |p: &str| default_of("Spectrum", p);
    assert_eq!(d("Block"), 5.0, "8192");
    assert_eq!(d("Refresh"), 60.0);
    assert_eq!(d("Avg"), 1.0);
    assert_eq!(d("Max"), 1.0);
    assert_eq!(d("Scale X"), 1.0, "Log");
    assert_eq!(d("Range"), 1.0, "Auto");
    assert_eq!((d("Range Max"), d("Range Min")), (0.0, -120.0));
}

#[test]
fn tuners_defaults_are_lives() {
    let d = |p: &str| default_of("Tuner", p);
    assert_eq!(d("Reference"), 440.0);
    assert_eq!(d("Classic Mode"), 0.0, "Target");
    assert_eq!(d("Readout"), 1.0, "ct");
}

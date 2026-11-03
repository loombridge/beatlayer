//! The distortion, resonator and spectral effects' panels as Live lays them
//! out (layout L-DEV-070 to L-DEV-084): each device's panel holds Live's
//! controls in Live's order and kinds and none else, and a new device
//! starts at the values Live's panel reads.
//!
//! Spec: device-framework R59; each device's spec, as each test names it.

use bl_model::device_layout::{PageChoice, Widget};
use bl_model::{DeviceDescription, ParamId};

use Widget::{
    Beside, BesideDim, BesideSwitch, Check, Chooser, Cycle, Grid, IconColumn, IconSegments,
    IconToggle, Knob, LitChooser, Marked, Number, Pill, Readout, Segments, Slider, SmallKnob,
    Stack, Toggle, Unboxed, UnboxedChooser,
};

fn description(device: &str) -> DeviceDescription {
    bl_devices::device_types()
        .into_iter()
        .find(|t| t.name == device)
        .unwrap_or_else(|| panic!("{device} is a device"))
        .description
}

/// What `device`'s panel holds with its pages `shown` (by node, then page)
/// and its expanded part closed: each control's parameter name and widget,
/// in the order the layout places them.
fn panel(device: &str, shown: &[(&str, &str)]) -> Vec<(String, Widget)> {
    let description = description(device);
    let choice: PageChoice = shown
        .iter()
        .map(|(node, page)| ((*node).to_owned(), (*page).to_owned()))
        .collect();
    let layout = description.layout.with_pages(&choice).shown(false);
    layout
        .place(layout.docked_size())
        .controls
        .iter()
        .map(|c| (description.param(c.param).unwrap().name.clone(), c.widget))
        .collect()
}

fn expect(device: &str, shown: &[(&str, &str)], live: &[(&str, Widget)]) {
    let live: Vec<(String, Widget)> = live.iter().map(|(n, w)| ((*n).to_owned(), *w)).collect();
    assert_eq!(panel(device, shown), live, "{device}'s panel");
}

/// The value `device` starts at for the parameter called `name`.
fn starts(device: &str, name: &str) -> f64 {
    let description = description(device);
    let id: ParamId = description
        .params
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{device} has {name}"))
        .id;
    description.param(id).unwrap().default
}

/// The text `device` shows for the parameter called `name` at its start.
fn reads(device: &str, name: &str) -> String {
    let description = description(device);
    let param = description
        .params
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{device} has {name}"));
    param.format(param.default)
}

// Spec: pedal R1, R7
#[test]
fn pedal_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Pedal",
        &[],
        &[
            ("Drive", SmallKnob),
            ("Output", SmallKnob),
            ("Type", Segments(3)),
            ("Bass", SmallKnob),
            ("Mid", SmallKnob),
            ("Treble", SmallKnob),
            ("Mid Frequency", Pill),
            ("Sub", Toggle),
            ("Dry/Wet", Beside(42)),
        ],
    );
}

// Spec: overdrive R1
#[test]
fn overdrive_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Overdrive",
        &[],
        &[
            ("Frequency", Number),
            ("Width", Number),
            ("Drive", SmallKnob),
            ("Tone", SmallKnob),
            ("Dynamics", Beside(53)),
            ("Dry/Wet", Beside(53)),
        ],
    );
}

// Spec: saturator R1
#[test]
fn saturator_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Saturator",
        &[],
        &[
            ("Curve", LitChooser),
            ("Post Clip", UnboxedChooser),
            ("Color", Toggle),
            ("Amt Lo", Readout),
            ("Drive", SmallKnob),
            ("Output", SmallKnob),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: tube-distortion R1
#[test]
fn dynamic_tube_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Dynamic Tube",
        &[],
        &[
            ("Dry/Wet", SmallKnob),
            ("Output", SmallKnob),
            ("Drive", SmallKnob),
            (
                "Tube",
                Grid {
                    columns: 3,
                    rows: 1,
                },
            ),
            ("Tone", SmallKnob),
            ("Bias", SmallKnob),
            ("Envelope", SmallKnob),
            ("Attack", SmallKnob),
            ("Release", SmallKnob),
        ],
    );
}

// Spec: bit-crusher R4
#[test]
fn redux_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Redux",
        &[],
        &[
            ("Rate", Knob),
            ("Jitter", SmallKnob),
            ("Pre", Toggle),
            ("Post", Toggle),
            ("Post Octave", Marked),
            ("Bits", Knob),
            ("Shape", SmallKnob),
            ("DC Shift", Toggle),
            ("Dry/Wet", Slider),
        ],
    );
}

// Spec: digital-degradation R4
#[test]
fn erosion_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Erosion",
        &[],
        &[
            ("Width", Readout),
            ("Stereo", Readout),
            ("Freq", SmallKnob),
            ("Noise Blend", Knob),
            ("Amount", SmallKnob),
        ],
    );
}

// Spec: vinyl-distortion R1, R2
#[test]
fn vinyl_distortion_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Vinyl Distortion",
        &[],
        &[
            ("Tracing On", Toggle),
            ("Tracing Drive", Number),
            ("Tracing Freq", Beside(24)),
            ("Tracing Width", Beside(9)),
            ("Soft/Hard", Segments(2)),
            ("Pinch Mode", Segments(2)),
            ("Pinch On", Toggle),
            ("Pinch Drive", Number),
            ("Pinch Freq", Beside(24)),
            ("Pinch Width", Beside(9)),
            ("Crackle Volume", SmallKnob),
            ("Crackle Density", SmallKnob),
            ("Drive", SmallKnob),
        ],
    );
}

// Spec: external-devices R17
#[test]
fn external_audio_effect_panel_holds_lives_controls_in_lives_order() {
    expect(
        "External Audio Effect",
        &[],
        &[
            ("Invert", Toggle),
            ("Out Gain", SmallKnob),
            ("In Gain", SmallKnob),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: resonators R1, R2
#[test]
fn resonators_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Resonators",
        &[],
        &[
            ("Filter On", Toggle),
            ("Filter Frequency", SmallKnob),
            ("Filter Type", IconColumn(4)),
            ("Mode", Cycle),
            ("Decay", SmallKnob),
            ("Const", Toggle),
            ("Color", SmallKnob),
            ("I On", Toggle),
            ("I Note", SmallKnob),
            ("I Fine", Number),
            ("I Gain", SmallKnob),
            ("II On", Toggle),
            ("II Pitch", SmallKnob),
            ("II Fine", Number),
            ("II Gain", SmallKnob),
            ("III On", Toggle),
            ("III Pitch", SmallKnob),
            ("III Fine", Number),
            ("III Gain", SmallKnob),
            ("IV On", Toggle),
            ("IV Pitch", SmallKnob),
            ("IV Fine", Number),
            ("IV Gain", SmallKnob),
            ("V On", Toggle),
            ("V Pitch", SmallKnob),
            ("V Fine", Number),
            ("V Gain", SmallKnob),
            ("Width", SmallKnob),
            ("Gain", SmallKnob),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: vocoder R1
#[test]
fn vocoder_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Vocoder",
        &[],
        &[
            ("Enhance", Toggle),
            ("Carrier", Chooser),
            ("Unvoiced", SmallKnob),
            ("Sens.", Slider),
            ("Fast/Slow", Cycle),
            ("Bands", UnboxedChooser),
            ("Range High", Unboxed),
            ("Range Low", Unboxed),
            ("BW", Unboxed),
            ("Precise/Retro", Cycle),
            ("Gate", Unboxed),
            ("Level", Unboxed),
            ("Depth", SmallKnob),
            ("Channel Mode", Stack(3)),
            ("Attack", SmallKnob),
            ("Formant", SmallKnob),
            ("Release", SmallKnob),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: pitch-and-frequency-shifter R1
#[test]
fn shifter_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Shifter",
        &[],
        &[
            ("Coarse", Knob),
            ("Fine", SmallKnob),
            ("Wide", Toggle),
            ("Window", Slider),
            ("Tone", Slider),
            ("Delay", Toggle),
            ("Delay Time", Number),
            ("Delay Mode", IconSegments(2)),
            ("Feedback", SmallKnob),
            ("LFO Stereo", Cycle),
            ("LFO Phase", Unboxed),
            ("Env Attack", Unboxed),
            ("Env Release", Unboxed),
            ("LFO Wave", Chooser),
            ("LFO Mode", IconSegments(2)),
            ("LFO Rate", SmallKnob),
            ("LFO Amount", SmallKnob),
            ("Env Fol", Toggle),
            ("Env Amount", Marked),
            ("Mode", Stack(3)),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: spectral-resonator R1
#[test]
fn spectral_resonator_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Spectral Resonator",
        &[],
        &[
            ("Pitch Mode", Segments(2)),
            ("Mono/Poly", Cycle),
            ("Polyphony", Number),
            ("MIDI Gate", BesideSwitch(47)),
            ("Glide", BesideDim(28)),
            ("PB", BesideDim(28)),
            ("Freq", Knob),
            ("Stretch", Slider),
            ("Shift", Slider),
            ("Decay", SmallKnob),
            ("Hi Damp", SmallKnob),
            ("Lo Damp", SmallKnob),
            ("Modulation", Stack(4)),
            ("Mod Rate", SmallKnob),
            ("Pch.Mod", Number),
            ("Harmonics", Readout),
            ("Quantize", Check),
            ("Input Send", SmallKnob),
            ("Unison", Number),
            ("Uni. Amt", Number),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: spectral-time R1
#[test]
fn spectral_time_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Spectral Time",
        &[],
        &[
            ("Freezer On", Check),
            ("Mode", Stack(2)),
            ("Trigger", Segments(2)),
            ("Fade Shape", IconSegments(2)),
            ("Fade In", SmallKnob),
            ("Fade Out", SmallKnob),
            ("Freeze", Toggle),
            ("Delay On", Check),
            ("Time", SmallKnob),
            ("Feedback", SmallKnob),
            ("Shift", SmallKnob),
            ("Delay Mode", Chooser),
            ("Stereo", Number),
            ("Delay Dry/Wet", Slider),
            ("Tilt", SmallKnob),
            ("Spray", SmallKnob),
            ("Mask", SmallKnob),
            ("Resolution", Readout),
            ("Input Send", SmallKnob),
            ("Order", Stack(2)),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: corpus R1
#[test]
fn corpus_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Corpus",
        &[],
        &[
            ("Type", UnboxedChooser),
            ("Quality", UnboxedChooser),
            ("Decay", Readout),
            ("Material", Readout),
            ("Width", Unboxed),
            ("Pos. L", Unboxed),
            ("Pos. R", Unboxed),
            ("Bright", SmallKnob),
            ("Inharm", SmallKnob),
            ("Ratio", SmallKnob),
            ("Hit", SmallKnob),
            ("Tune", SmallKnob),
            ("Fine", SmallKnob),
            ("Spread", Slider),
            ("LFO", Check),
            ("LFO Amount", SmallKnob),
            ("LFO Wave", Chooser),
            ("LFO Rate", SmallKnob),
            ("LFO Mode", IconColumn(2)),
            ("LFO Phase", SmallKnob),
            ("LFO Stereo", IconColumn(2)),
            ("Filter", Check),
            ("Filter Freq", SmallKnob),
            ("Bdwidth", SmallKnob),
            ("Bleed", SmallKnob),
            ("Gain", SmallKnob),
            ("Dry/Wet", SmallKnob),
        ],
    );
}

// Spec: roar R1
#[test]
fn roar_panel_holds_lives_controls_in_lives_order() {
    expect(
        "Roar",
        &[],
        &[
            ("Drive", Knob),
            ("Tone Amount", SmallKnob),
            ("Color Compensation", IconToggle),
            ("Tone Frequency", Slider),
            ("Routing Mode", UnboxedChooser),
            ("Low Crossover", Unboxed),
            ("High Crossover", Unboxed),
            ("Stage 1 Amount", Knob),
            ("Stage 1 Bias", SmallKnob),
            ("Stage 1 Frequency", Knob),
            ("Stage 1 Pre", Toggle),
            ("Stage 1", Check),
            ("Stage 1 Shaper", UnboxedChooser),
            ("Stage 1 Level", Unboxed),
            ("Stage 1 Filter", UnboxedChooser),
            ("Stage 1 Resonance", Unboxed),
            ("Feedback Mode", Chooser),
            ("Feedback Time", Slider),
            ("Feedback Amount", SmallKnob),
            ("Feedback Invert", Toggle),
            ("Feedback Gate", IconToggle),
            ("Feedback Frequency", Slider),
            ("Feedback Width", Slider),
            ("Compression Amount", SmallKnob),
            ("Sidechain HP", Toggle),
            ("Output Gain", SmallKnob),
            ("Dry/Wet", Slider),
        ],
    );
    // The other two stages' pages hold the same controls, each its own
    // stage's.
    let low = panel("Roar", &[]);
    for stage in ["Mid", "High"] {
        let shown = panel("Roar", &[("Stage", stage)]);
        let kinds = |panel: &[(String, Widget)]| panel.iter().map(|(_, w)| *w).collect::<Vec<_>>();
        assert_eq!(kinds(&shown), kinds(&low), "{stage}'s page");
        assert_ne!(shown, low, "{stage} has its own stage's parameters");
    }
}

/// A new device's values, as Live's panel reads them (observed in Live
/// 12.4.16, 2 November 2026): each parameter's start as a number, with the
/// text it reads as where the unit matters. A gain's number is the linear
/// factor, 1 for 0 dB and 0 for −∞.
const STARTS: &[(&str, &[(&str, f64)])] = &[
    (
        "Pedal",
        &[
            ("Drive", 0.0),
            ("Output", 1.0),
            ("Type", 0.0),
            ("Bass", 0.0),
            ("Mid", 0.0),
            ("Treble", 0.0),
            ("Sub", 0.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Overdrive",
        &[
            ("Frequency", 1_250.0),
            ("Width", 6.5),
            ("Drive", 50.0),
            ("Tone", 50.0),
            ("Dynamics", 50.0),
            ("Dry/Wet", 50.0),
        ],
    ),
    (
        "Saturator",
        &[
            ("Curve", 0.0),
            ("Post Clip", 0.0),
            ("Color", 1.0),
            ("Amt Lo", 0.0),
            ("Drive", 0.0),
            ("Output", 1.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Dynamic Tube",
        &[
            ("Dry/Wet", 100.0),
            ("Output", 1.0),
            ("Drive", 0.0),
            ("Tube", 0.0),
            ("Tone", 0.0),
            ("Bias", 0.0),
            ("Envelope", 0.0),
            ("Attack", 5.0),
            ("Release", 35.0),
        ],
    ),
    (
        "Redux",
        &[
            ("Rate", 40_000.0),
            ("Jitter", 0.0),
            ("Pre", 0.0),
            ("Post", 0.0),
            ("Post Octave", 0.0),
            ("Bits", 16.0),
            ("Shape", 0.0),
            ("DC Shift", 0.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Erosion",
        &[
            ("Width", 1.0),
            ("Stereo", 25.0),
            ("Freq", 500.0),
            ("Noise Blend", 50.0),
            ("Amount", 20.0),
        ],
    ),
    (
        "Vinyl Distortion",
        &[
            ("Tracing On", 1.0),
            ("Tracing Drive", 0.0),
            ("Tracing Freq", 684.0),
            ("Tracing Width", 0.32),
            ("Soft/Hard", 0.0),
            ("Pinch Mode", 0.0),
            ("Pinch On", 1.0),
            ("Pinch Drive", 0.0),
            ("Pinch Freq", 7_500.0),
            ("Pinch Width", 3.0),
            ("Crackle Volume", 0.0),
            ("Crackle Density", 10.0),
            ("Drive", 1.0),
        ],
    ),
    (
        "External Audio Effect",
        &[
            ("Out Gain", 1.0),
            ("In Gain", 1.0),
            ("Dry/Wet", 100.0),
            ("Invert", 0.0),
        ],
    ),
    (
        "Resonators",
        &[
            ("Filter On", 1.0),
            ("Filter Frequency", 500.0),
            ("Filter Type", 2.0),
            ("Mode", 0.0),
            ("Decay", 50.0),
            ("Const", 1.0),
            ("Color", 90.0),
            ("I On", 1.0),
            ("I Note", 48.0),
            ("I Fine", 0.0),
            ("I Gain", 1.0),
            ("II On", 1.0),
            ("II Pitch", 0.0),
            ("III Pitch", 0.0),
            ("IV Pitch", 0.0),
            ("V On", 1.0),
            ("V Pitch", 0.0),
            ("V Gain", 1.0),
            ("Width", 100.0),
            ("Gain", 1.0),
            ("Dry/Wet", 50.0),
        ],
    ),
    (
        "Vocoder",
        &[
            ("Carrier", 0.0),
            ("Enhance", 0.0),
            ("Unvoiced", 0.0),
            ("Sens.", 50.0),
            ("Fast/Slow", 0.0),
            ("Bands", 4.0),
            ("Range High", 12_000.0),
            ("Range Low", 80.0),
            ("BW", 100.0),
            ("Gate", 0.0),
            ("Level", 1.0),
            ("Depth", 100.0),
            ("Channel Mode", 1.0),
            ("Attack", 1.0),
            ("Formant", 0.0),
            ("Release", 150.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Shifter",
        &[
            ("Mode", 0.0),
            ("Coarse", 0.0),
            ("Fine", 0.0),
            ("Wide", 0.0),
            ("Tone", 22_000.0),
            ("Delay", 0.0),
            ("Delay Mode", 1.0),
            ("Feedback", 0.0),
            ("LFO Wave", 1.0),
            ("LFO Mode", 0.0),
            ("LFO Rate", 0.5),
            ("LFO Amount", 0.0),
            ("LFO Stereo", 1.0),
            ("LFO Spin", 0.0),
            ("Env Fol", 0.0),
            ("Env Attack", 6.0),
            ("Env Release", 200.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Spectral Resonator",
        &[
            ("Pitch Mode", 0.0),
            ("Mono/Poly", 0.0),
            ("Polyphony", 8.0),
            ("MIDI Gate", 0.0),
            ("Glide", 0.0),
            ("PB", 2.0),
            ("Freq", 110.0),
            ("Stretch", 0.0),
            ("Shift", 0.0),
            ("Decay", 222.0),
            ("Hi Damp", 0.0),
            ("Lo Damp", 0.0),
            ("Modulation", 0.0),
            ("Mod Rate", 10.0),
            ("Pch.Mod", 0.0),
            ("Quantize", 0.0),
            ("Input Send", 1.0),
            ("Unison", 1.0),
            ("Uni. Amt", 10.0),
            ("Dry/Wet", 100.0),
        ],
    ),
    (
        "Spectral Time",
        &[
            ("Freezer On", 1.0),
            ("Mode", 0.0),
            ("Trigger", 0.0),
            ("Fade In", 2.0),
            ("Fade Out", 40.0),
            ("Delay On", 1.0),
            ("Time", 250.0),
            ("Feedback", 0.0),
            ("Shift", 0.0),
            ("Delay Mode", 0.0),
            ("Stereo", 0.0),
            ("Delay Dry/Wet", 50.0),
            ("Tilt", 0.0),
            ("Spray", 0.0),
            ("Mask", 0.0),
            ("Input Send", 1.0),
            ("Order", 0.0),
            ("Dry/Wet", 50.0),
        ],
    ),
    (
        "Corpus",
        &[
            ("Type", 0.0),
            ("Decay", 1.32),
            ("Material", 0.0),
            ("Width", 100.0),
            ("Pos. L", 10.0),
            ("Pos. R", 90.0),
            ("Bright", 0.0),
            ("Inharm", 0.0),
            ("Ratio", 50.0),
            ("Hit", 50.0),
            ("Tune", 261.63),
            ("Fine", 0.0),
            ("Spread", 0.0),
            ("LFO", 0.0),
            ("LFO Amount", 0.0),
            ("LFO Rate", 0.32),
            ("LFO Phase", 180.0),
            ("Filter", 0.0),
            ("Filter Freq", 1_000.0),
            ("Bdwidth", 4.0),
            ("Bleed", 0.0),
            ("Gain", 1.0),
            ("Dry/Wet", 50.0),
        ],
    ),
    (
        "Roar",
        &[
            ("Drive", 0.0),
            ("Tone Amount", 0.0),
            ("Tone Frequency", 180.0),
            ("Routing Mode", 3.0),
            ("Low Crossover", 200.0),
            ("High Crossover", 2_000.0),
            ("Stage 3 Amount", 0.0),
            ("Stage 3 Bias", 0.0),
            ("Stage 3 Frequency", 16_000.0),
            ("Stage 3 Resonance", 10.0),
            ("Stage 3 Level", 1.0),
            ("Feedback Mode", 0.0),
            ("Feedback Time", 18.2),
            ("Feedback Amount", 0.0),
            ("Feedback Frequency", 1_000.0),
            ("Feedback Width", 8.0),
            ("Compression Amount", 25.0),
            ("Sidechain HP", 1.0),
            ("Output Gain", 1.0),
            ("Dry/Wet", 100.0),
        ],
    ),
];

// Spec: each device's spec (the defaults), device-framework R59
#[test]
fn a_new_device_starts_at_the_values_lives_panel_reads() {
    for (device, starts_at) in STARTS {
        for (name, value) in *starts_at {
            let found = starts(device, name);
            assert!(
                (found - value).abs() <= 1e-9 * value.abs().max(1.0),
                "{device}'s {name} starts at {found}, Live's panel reads {value}"
            );
        }
    }
}

// Spec: pedal R2, R7, saturator R1, spectral-resonator R3, corpus R3
#[test]
fn the_units_are_the_ones_lives_panel_reads() {
    for (device, name, text) in [
        ("Pedal", "Drive", "0.0 %"),
        ("Pedal", "Bass", "0.0 %"),
        ("Pedal", "Output", "0.00 dB"),
        ("Overdrive", "Frequency", "1.25 kHz"),
        ("Saturator", "Drive", "0.00 dB"),
        ("Redux", "Rate", "40.0 kHz"),
        ("Redux", "Bits", "16"),
        ("Erosion", "Freq", "500 Hz"),
        ("Dynamic Tube", "Attack", "5.00 ms"),
        ("Vinyl Distortion", "Tracing Freq", "684 Hz"),
        ("Vinyl Distortion", "Pinch Freq", "7.50 kHz"),
        ("Vocoder", "Release", "150 ms"),
        ("Vocoder", "Unvoiced", "-inf dB"),
        ("Vocoder", "Gate", "-inf dB"),
        ("Shifter", "Tone", "22.0 kHz"),
        ("Shifter", "Delay Time", "200 ms"),
        ("Spectral Resonator", "Decay", "222 ms"),
        ("Spectral Resonator", "Mod Rate", "10 %"),
        ("Spectral Time", "Fade Out", "40.0 ms"),
        ("Corpus", "Decay", "1.32 s"),
        ("Roar", "Feedback Time", "18.2 ms"),
    ] {
        assert_eq!(reads(device, name), text, "{device}'s {name}");
    }
}

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
const STILL_GENERIC: &[&str] = &["External Audio Effect", "External Instrument", "Utility"];

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

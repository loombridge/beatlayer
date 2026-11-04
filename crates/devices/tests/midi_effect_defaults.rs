//! A new MIDI effect starts as Live's panel reads in a new set (observed in
//! Live 12.4.16 on 2 November 2026; layout L-DEV-063 to L-DEV-069 and
//! L-DEV-098 to L-DEV-101): each parameter's default, as its control reads it. Live writes a few of these
//! with a sign (`+12 st`) or with `∞`, which the parameter formats don't.

use bl_devices::device_types;

/// What `device`'s parameter `name` reads at its default.
fn reads(device: &str, name: &str) -> String {
    let t = device_types()
        .into_iter()
        .find(|t| t.name == device)
        .unwrap_or_else(|| panic!("{device} is a device"));
    let d = t
        .description
        .params
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{device} has a {name}"));
    d.format(d.default)
}

fn expect(device: &str, live: &[(&str, &str)]) {
    for (name, text) in live {
        assert_eq!(reads(device, name), *text, "{device}'s {name}");
    }
}

#[test]
fn a_new_arpeggiator_starts_as_lives_does() {
    expect(
        "Arpeggiator",
        &[
            ("Style", "Up"),
            ("Hold", "Off"),
            ("Offset", "0"),
            ("Groove", "Straight"),
            ("Sync", "On"),
            ("Rate Sync", "1/8"),
            ("Distance", "12 st"),
            ("Steps", "0"),
            ("Gate", "50 %"),
            ("Retrigger", "Off"),
            ("Interval", "1"),
            ("Repeats", "0"),
            ("Velocity", "Off"),
            ("Decay", "1.00 s"),
            ("Target", "64"),
            ("Retrig Vel", "Off"),
            ("Root", "C"),
            ("Scale", "Chromatic"),
        ],
    );
}

#[test]
fn a_new_cc_control_starts_as_lives_does() {
    expect(
        "CC Control",
        &[
            ("Mod Wheel", "0"),
            ("Pitch Bend", "0"),
            ("Pressure", "0"),
            ("Custom A", "0"),
            ("Custom A CC", "64: Sustain Pedal"),
            ("Custom B", "0"),
            ("Custom B CC", "None"),
            ("Custom E CC", "None"),
        ],
    );
}

#[test]
fn a_new_chord_adds_no_notes_and_shows_velocities() {
    for n in 1..=6 {
        expect(
            "Chord",
            &[
                (&format!("Shift {n}"), "0 st"),
                (&format!("Velocity {n}"), "100 %"),
                (&format!("Chance {n}"), "100 %"),
            ],
        );
    }
    expect(
        "Chord",
        &[
            ("Strum", "0 ms"),
            ("Tension", "0 %"),
            ("Crescendo", "0 %"),
            ("Amounts", "Velocity"),
        ],
    );
}

#[test]
fn a_new_mpe_control_starts_with_a_straight_curve_over_the_whole_range() {
    for source in ["Press", "Slide", "NotePB"] {
        // NotePB starts with the S-shaped curve (observed 3 November 2026).
        let curve = if source == "NotePB" {
            "S-shaped"
        } else {
            "Linear"
        };
        expect(
            "MPE Control",
            &[
                (&format!("{source} Curve"), curve),
                (&format!("{source} Curve A"), "0.00"),
                (&format!("{source} Curve B"), "0.00"),
                (&format!("{source} Min"), "0"),
                (&format!("{source} Max"), "127"),
                (&format!("{source} X"), "64"),
                (&format!("{source} Y"), "64"),
                (&format!("{source} Smooth"), "Off"),
                (&format!("{source} Rise"), "15.0 ms"),
                (&format!("{source} Fall"), "15.0 ms"),
            ],
        );
    }
    expect(
        "MPE Control",
        &[
            ("Press Default Value", "0"),
            ("Slide Default Value", "0"),
            ("Pitch Range", "1x"),
        ],
    );
}

#[test]
fn a_new_note_echo_starts_as_lives_does() {
    expect(
        "Note Echo",
        &[
            ("Delay Time", "2"),
            ("Sync", "Sync"),
            ("Swing", "0.00 %"),
            ("Time Free", "500 ms"),
            ("Input", "Thru"),
            ("Pitch", "0 st"),
            ("Delay", "100 %"),
            ("Fback", "50.0 %"),
            ("MPE", "Off"),
            ("Press", "100 %"),
            ("Slide", "100 %"),
            ("Note PB", "100 %"),
        ],
    );
}

#[test]
fn a_new_note_length_starts_as_lives_does() {
    expect(
        "Note Length",
        &[
            ("Trigger", "Note On"),
            ("Latch", "Off"),
            ("Gate", "100 %"),
            ("Sync", "Off"),
            ("Length", "100 ms"),
            ("Rel Vel", "0 %"),
            ("Decay", "60.0 s"),
            ("Key Scale", "0.00"),
        ],
    );
}

#[test]
fn a_new_pitch_starts_as_lives_does() {
    expect(
        "Pitch",
        &[
            ("Pitch", "0 st"),
            ("Step Width", "12 st"),
            ("Lowest", "C-2"),
            ("Range", "127 st"),
            ("Mode", "Block"),
        ],
    );
}

#[test]
fn a_new_random_starts_as_lives_does() {
    expect(
        "Random",
        &[
            ("Chance", "0.0 %"),
            ("Choices", "12"),
            ("Interval", "1"),
            ("Mode", "Random"),
            ("Sign", "Add"),
        ],
    );
}

#[test]
fn a_new_scale_starts_as_lives_does() {
    expect(
        "Scale",
        &[
            ("Base", "C"),
            ("Scale Name", "User"),
            ("Transpose", "0 st"),
            ("Fold", "Off"),
            ("Lowest", "C-2"),
            ("Range", "128 st"),
        ],
    );
}

#[test]
fn a_new_velocity_starts_as_lives_does() {
    expect(
        "Velocity",
        &[
            ("Lowest", "1"),
            ("Range", "127"),
            ("Out Low", "1"),
            ("Out Hi", "127"),
            ("Operation", "Both"),
            ("Mode", "Clip"),
            ("Random", "0"),
            ("Drive", "0.00"),
            ("Comp.", "0.00"),
        ],
    );
}

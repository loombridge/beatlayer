//! What Wavetable's panel draws (WP-8.11; the wavetable spec, R35 and R38),
//! read headless as `device.display` reads it: each oscillator's table
//! decimated to a fixed size with the frame at Position, the filters'
//! combined response with a dot each, the envelopes' shapes with their
//! handles, and the LFOs' waves.

use bl_core::id::Id;
use bl_devices::wavetable::{
    ENV_ATTACK, ENV_DECAY, ENV_RELEASE, ENV_SUSTAIN, FILTER_FREQ, FILTER_ON, FILTER_RES,
    LFO_AMOUNT, LFO_SHAPE, LFO_WAVE, OSC_ON, POSITION, ROUTING, envelope, filter, lfo, osc,
    wavetable_type,
};
use bl_devices::wavetable_display::{
    ATTACK_SPAN, DECAY_SPAN, POINTS, Preview, RELEASE_SPAN, TABLE_FRAMES, display,
};
use bl_model::{DeviceDisplay, DeviceId, OrdMap, ParamId};
use lspk_wavetable::Wavetable;

fn shown(values: &[(ParamId, f64)], tables: [Option<&Preview>; 2]) -> DeviceDisplay {
    let description = wavetable_type().description;
    let mut set = OrdMap::new();
    for (param, value) in values {
        set.insert(*param, *value);
    }
    display(
        DeviceId(Id::new(1).unwrap()),
        &description,
        &set,
        None,
        tables,
    )
}

fn factory(name: &str) -> Preview {
    let table = bl_devices::wavetables::factory("Shape Morphs", name)
        .or_else(|| bl_devices::wavetables::factory("FM", name))
        .expect("a factory table");
    Preview::of(&table.source())
}

/// A table of `frames` frames, each a different constant.
fn ramp(frames: usize) -> Preview {
    let table = Wavetable::from_fn(frames, |frame, _| frame as f32 / frames as f32).unwrap();
    Preview::of(&table)
}

// Spec: wavetable R35
#[test]
fn an_oscillators_table_is_decimated_to_32_frames_of_128_points_with_the_frame_at_position() {
    let sine_to_saw = factory("Sine to Saw");
    assert_eq!(sine_to_saw.frames(), 64);
    let at = |position: f64| shown(&[(osc(1, POSITION), position)], [Some(&sine_to_saw), None]);
    let display = at(0.0);
    let tables: Vec<_> = display
        .series
        .iter()
        .filter(|s| s.name.starts_with("Osc 1 Table "))
        .collect();
    assert_eq!(tables.len(), TABLE_FRAMES, "32 of the 64 frames");
    assert!(tables.iter().all(|s| s.values.len() == POINTS && s.detail));
    assert_eq!(tables[0].name, "Osc 1 Table 1");
    // The frame at Position is its own series, in the accent (not detail),
    // and is the table's first frame at 0 % and its last at 100 %.
    let frame = |d: &DeviceDisplay| d.series("Osc 1 Frame").unwrap().clone();
    assert!(!frame(&display).detail);
    assert_eq!(frame(&display).values, tables[0].values);
    let last = at(100.0);
    let last_tables: Vec<_> = last
        .series
        .iter()
        .filter(|s| s.name.starts_with("Osc 1 Table "))
        .collect();
    assert_eq!(frame(&last).values, last_tables[TABLE_FRAMES - 1].values);
    assert_ne!(frame(&display).values, frame(&last).values);
    // Halfway is a blend of the two frames beside it.
    assert_eq!(frame(&at(50.0)).values, sine_to_saw.at(0.5).to_vec());
    let reading = at(25.0).reading("Osc 1 Position").unwrap().clone();
    assert_eq!(reading.text, "25.0 %, frame 17 of 64");
    assert!(
        at(25.0)
            .text()
            .contains("Osc 1 Position 25.0 %, frame 17 of 64")
    );
}

// Spec: wavetable R35, device framework R35
#[test]
fn the_display_keeps_its_size_whatever_the_table() {
    let counts = |preview: &Preview| {
        let d = shown(&[], [Some(preview), None]);
        let tables = d
            .series
            .iter()
            .filter(|s| s.name.starts_with("Osc 1 Table "))
            .count();
        (tables, d.series.iter().all(|s| s.values.len() <= POINTS))
    };
    assert_eq!(
        counts(&ramp(256)),
        (TABLE_FRAMES, true),
        "a 256-frame table"
    );
    assert_eq!(counts(&ramp(1)), (1, true), "a one-frame table");
    assert_eq!(
        counts(&ramp(8)),
        (8, true),
        "a short table draws all its frames"
    );
    // A table not yet built draws no frames, and says so in the reading.
    let none = shown(&[], [None, None]);
    assert!(none.series.iter().all(|s| !s.name.starts_with("Osc 1 ")));
    assert_eq!(none.reading("Osc 1 Position").unwrap().text, "0.0 %");
    // An oscillator that is off draws no frame at Position.
    let sine_to_saw = factory("Sine to Saw");
    let off = shown(&[(osc(1, OSC_ON), 0.0)], [Some(&sine_to_saw), None]);
    assert!(off.series("Osc 1 Frame").is_none());
}

fn at_hz(display: &DeviceDisplay, name: &str, hz: f32) -> f32 {
    let series = display.series(name).unwrap();
    let nearest = (0..series.values.len())
        .min_by(|a, b| {
            (series.x_at(*a) - hz)
                .abs()
                .total_cmp(&(series.x_at(*b) - hz).abs())
        })
        .unwrap();
    series.values[nearest]
}

// Spec: wavetable R38
#[test]
fn the_filter_display_is_the_two_filters_response_as_the_routing_makes_it() {
    let both = [
        (filter(1, FILTER_ON), 1.0),
        (filter(2, FILTER_ON), 1.0),
        (filter(1, FILTER_FREQ), 800.0),
        (filter(2, FILTER_FREQ), 3_000.0),
    ];
    let serial = shown(&both, [None, None]);
    // Serial: the two responses add in dB.
    let (one, two) = (
        serial.series("Filter 1").unwrap(),
        serial.series("Filter 2").unwrap(),
    );
    let sum = serial.series("Filter").unwrap();
    for i in (0..POINTS).step_by(8) {
        let expected = one.values[i] + two.values[i];
        // The display's floor is -120 dB.
        if expected > -100.0 {
            assert!(
                (sum.values[i] - expected).abs() < 0.05,
                "{i}: {} vs {expected}",
                sum.values[i]
            );
        }
    }
    assert!(
        at_hz(&serial, "Filter", 20.0).abs() < 0.5,
        "passes the lows"
    );
    assert!(at_hz(&serial, "Filter", 15_000.0) < -40.0, "cuts the highs");
    // Parallel: two filters that agree are one filter.
    let mut same = both.to_vec();
    same[3].1 = 800.0;
    same.push((ROUTING, 1.0));
    let parallel = shown(&same, [None, None]);
    for hz in [100.0, 800.0, 3_000.0] {
        let (all, first) = (
            at_hz(&parallel, "Filter", hz),
            at_hz(&parallel, "Filter 1", hz),
        );
        assert!((all - first).abs() < 0.05, "{hz} Hz: {all} vs {first}");
    }
    // A filter that is off passes everything and has no dot.
    let one_on = shown(&[(filter(1, FILTER_ON), 1.0)], [None, None]);
    assert!(one_on.series("Filter 2").is_none());
    assert_eq!(
        one_on
            .markers
            .iter()
            .filter(|m| m.series == "Filter")
            .count(),
        1
    );
}

// Spec: wavetable R38
#[test]
fn each_filter_has_a_dot_that_drags_its_frequency_and_resonance() {
    let d = shown(
        &[
            (filter(1, FILTER_ON), 1.0),
            (filter(2, FILTER_ON), 1.0),
            (filter(1, FILTER_FREQ), 800.0),
            (filter(1, FILTER_RES), 40.0),
        ],
        [None, None],
    );
    let dot = d.markers.iter().find(|m| m.name == "Filter 1").unwrap();
    assert_eq!(dot.x, 800.0);
    let handle = dot.handle.unwrap();
    assert_eq!(handle.x, filter(1, FILTER_FREQ));
    assert_eq!(handle.y, Some(filter(1, FILTER_RES)));
    assert!(
        dot.text.contains("800 Hz") && dot.text.contains("Resonance 40 %"),
        "{}",
        dot.text
    );
    let dots = d.markers.iter().filter(|m| m.series == "Filter").count();
    assert_eq!(dots, 2);
}

// Spec: wavetable R38
#[test]
fn an_envelopes_display_draws_its_stages_with_a_handle_on_each_breakpoint() {
    let d = shown(
        &[
            (envelope(0, ENV_SUSTAIN), 50.0),
            (envelope(1, ENV_SUSTAIN), 50.0),
        ],
        [None, None],
    );
    let amp = d.series("Amp Envelope").unwrap();
    assert_eq!(amp.values.len(), POINTS);
    assert_eq!(amp.values[0], 0.0, "starts from silence");
    let top = amp.values.iter().copied().fold(0.0f32, f32::max);
    assert!((top - 1.0).abs() < 0.02, "rises to the peak: {top}");
    let held = (RELEASE_SPAN[0] * (POINTS - 1) as f32) as usize - 1;
    assert!((amp.values[held] - 0.5).abs() < 1e-3, "holds Sustain");
    assert!(amp.values[POINTS - 1].abs() < 1e-3, "falls to silence");
    let names: Vec<&str> = d
        .markers
        .iter()
        .filter(|m| m.series == "Amp Envelope")
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Amp Envelope Attack",
            "Amp Envelope Decay",
            "Amp Envelope Release"
        ]
    );
    let handle = |name: &str| {
        d.markers
            .iter()
            .find(|m| m.name == name)
            .unwrap()
            .handle
            .unwrap()
    };
    let decay = handle("Amp Envelope Decay");
    assert_eq!(
        (decay.x, decay.y),
        (envelope(0, ENV_DECAY), Some(envelope(0, ENV_SUSTAIN)))
    );
    assert_eq!(decay.span, Some(DECAY_SPAN));
    assert_eq!(handle("Amp Envelope Attack").span, Some(ATTACK_SPAN));
    assert_eq!(handle("Amp Envelope Attack").x, envelope(0, ENV_ATTACK));
    assert_eq!(handle("Amp Envelope Release").x, envelope(0, ENV_RELEASE));
    // Envelopes 2 and 3 add their levels to the handles' vertical places.
    assert!(handle("Env 2 Attack").y.is_some() && handle("Env 2 Release").y.is_some());
    assert!(handle("Amp Envelope Attack").y.is_none());
}

// Spec: wavetable R38
#[test]
fn an_lfos_display_draws_one_cycle_of_its_wave_at_its_amount() {
    let square = |amount: f64| {
        shown(
            &[
                (lfo(1, LFO_WAVE), 3.0),
                (lfo(1, LFO_SHAPE), 0.0),
                (lfo(1, LFO_AMOUNT), amount),
            ],
            [None, None],
        )
    };
    let full = square(100.0);
    let wave = full.series("LFO 1").unwrap();
    assert_eq!((wave.values[1], wave.values[POINTS - 2]), (1.0, -1.0));
    let half = square(50.0);
    assert_eq!(half.series("LFO 1").unwrap().values[1], 0.5);
    assert_eq!(full.reading("LFO 1 Wave").unwrap().text, "Square, 1.00 Hz");
}

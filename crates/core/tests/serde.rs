//! The serialised form of the time types, with the `serde` feature.

#![cfg(feature = "serde")]

use bl_core::time::{
    Bpm, Frames, SampleRate, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange,
    TimeSignatureMap,
};
use serde_json::json;

#[test]
fn integers_are_plain_numbers() {
    assert_eq!(
        serde_json::to_value(Ticks(705_600)).unwrap(),
        json!(705_600)
    );
    assert_eq!(serde_json::to_value(Frames(-3)).unwrap(), json!(-3));
    assert_eq!(
        serde_json::to_value(SampleRate::HZ_48000).unwrap(),
        json!(48_000)
    );
    assert!(serde_json::from_value::<SampleRate>(json!(0)).is_err());
}

#[test]
fn tempo_map_round_trip() {
    let map = TempoMap::new([
        TempoSegment::Ramp {
            start: Ticks::ZERO,
            from: Bpm::new(90.0).unwrap(),
            to: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(16),
            bpm: Bpm::new(120.0).unwrap(),
        },
    ])
    .unwrap();
    let value = serde_json::to_value(&map).unwrap();
    assert_eq!(
        value,
        json!([
            { "kind": "ramp", "start": 0, "from": 90.0, "to": 120.0 },
            { "kind": "constant", "start": 11_289_600, "bpm": 120.0 },
        ])
    );
    let back: TempoMap = serde_json::from_value(value).unwrap();
    assert_eq!(back, map);
    assert_eq!(
        back.ticks_to_seconds(Ticks::from_quarters(20)),
        map.ticks_to_seconds(Ticks::from_quarters(20))
    );
}

#[test]
fn invalid_tempo_maps_are_rejected() {
    // A tempo out of range.
    assert!(
        serde_json::from_value::<TempoMap>(json!([
            { "kind": "constant", "start": 0, "bpm": 0.0 },
        ]))
        .is_err()
    );
    // No segment at zero.
    assert!(
        serde_json::from_value::<TempoMap>(json!([
            { "kind": "constant", "start": 5, "bpm": 120.0 },
        ]))
        .is_err()
    );
    // A ramp at the end.
    assert!(
        serde_json::from_value::<TempoMap>(json!([
            { "kind": "ramp", "start": 0, "from": 90.0, "to": 120.0 },
        ]))
        .is_err()
    );
}

#[test]
fn time_signature_map_round_trip() {
    let map = TimeSignatureMap::new([
        TimeSignatureChange {
            at: Ticks::ZERO,
            signature: TimeSignature::FOUR_FOUR,
        },
        TimeSignatureChange {
            at: Ticks::from_quarters(8),
            signature: TimeSignature::new(7, 8).unwrap(),
        },
    ])
    .unwrap();
    let value = serde_json::to_value(&map).unwrap();
    assert_eq!(
        value,
        json!([
            { "at": 0, "signature": { "numerator": 4, "denominator": 4 } },
            { "at": 5_644_800, "signature": { "numerator": 7, "denominator": 8 } },
        ])
    );
    let back: TimeSignatureMap = serde_json::from_value(value).unwrap();
    assert_eq!(back, map);

    assert!(
        serde_json::from_value::<TimeSignature>(json!({ "numerator": 4, "denominator": 3 }))
            .is_err()
    );
    assert!(serde_json::from_value::<TimeSignatureMap>(json!([])).is_err());
}

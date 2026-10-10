//! Property tests for time-signature maps and bar-beat-sixteenth positions.
//!
//! Each property runs 10,000 cases. Maps have up to nine signatures, with
//! changes on bar lines and off them (fragmentary bars), including changes
//! that don't fall on a sixteenth.

use bl_core::time::{
    BarBeatSixteenth, TICKS_PER_QUARTER, Ticks, TimeSignature, TimeSignatureChange,
    TimeSignatureMap,
};
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::FileFailurePersistence;

const CASES: u32 = 10_000;

/// 10,000 cases each. Failures are saved next to this file and replayed
/// first on later runs.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: CASES,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}
const Q: i64 = TICKS_PER_QUARTER;
/// Positions run to a million quarter notes either side of zero.
const POSITIONS: i64 = 1_000_000 * Q;

fn signature() -> impl Strategy<Value = TimeSignature> {
    (1u8..=99, prop::sample::select(vec![1u8, 2, 4, 8, 16]))
        .prop_map(|(numerator, denominator)| TimeSignature::new(numerator, denominator).unwrap())
}

/// A map of up to nine signatures. Each change comes a whole number of bars
/// after the one before, or any number of ticks up to two whole notes.
fn signature_map() -> impl Strategy<Value = TimeSignatureMap> {
    let gap = prop_oneof![
        (1i64..=64).prop_map(Gap::Bars),
        (1i64..=8 * Q).prop_map(Gap::Ticks),
    ];
    (
        signature(),
        prop::collection::vec((gap, signature()), 0..=8),
    )
        .prop_map(|(first, rest)| {
            let mut changes = vec![TimeSignatureChange {
                at: Ticks::ZERO,
                signature: first,
            }];
            for (gap, signature) in rest {
                let before = changes.last().unwrap();
                let at = before.at
                    + match gap {
                        Gap::Bars(bars) => before.signature.bar_length() * bars,
                        Gap::Ticks(ticks) => Ticks(ticks),
                    };
                changes.push(TimeSignatureChange { at, signature });
            }
            TimeSignatureMap::new(changes).unwrap()
        })
}

#[derive(Clone, Copy, Debug)]
enum Gap {
    Bars(i64),
    Ticks(i64),
}

/// A map and a position: half near a change, half anywhere.
fn map_and_position() -> impl Strategy<Value = (TimeSignatureMap, Ticks)> {
    (
        signature_map(),
        any::<Index>(),
        -2 * Q..=2 * Q,
        -POSITIONS..=POSITIONS,
        any::<bool>(),
    )
        .prop_map(|(map, index, offset, anywhere, near_change)| {
            let ticks = if near_change {
                let changes: Vec<Ticks> = map.changes().map(|change| change.at).collect();
                changes[index.index(changes.len())] + Ticks(offset)
            } else {
                Ticks(anywhere)
            };
            (map, ticks)
        })
}

proptest! {
    #![proptest_config(config())]

    /// Ticks to bar-beat-sixteenth and back is exact, every field is in
    /// range, and the displayed text reads back as the start of the
    /// sixteenth.
    #[test]
    fn bbs_round_trip((map, ticks) in map_and_position()) {
        let position = map.to_bbs(ticks);
        let signature = map.signature_at(ticks);
        prop_assert!((1..=u32::from(signature.numerator())).contains(&position.beat));
        prop_assert!((1..=signature.sixteenths_per_beat()).contains(&position.sixteenth));
        prop_assert!(i64::from(position.ticks) < Ticks::SIXTEENTH.get());
        prop_assert_eq!(map.from_bbs(position), Ok(ticks), "via {:?}", position);

        let text = position.to_string();
        let sixteenth_start = ticks - Ticks(i64::from(position.ticks));
        prop_assert_eq!(map.parse_position(&text), Ok(sixteenth_start), "via {}", text);
    }

    /// Later ticks always show a later bar-beat-sixteenth position.
    #[test]
    fn bbs_is_monotonic(
        (map, ticks) in map_and_position(),
        step in prop_oneof![1i64..=2, 1i64..=Q, 1i64..=64 * Q],
    ) {
        prop_assert!(map.to_bbs(ticks) < map.to_bbs(ticks + Ticks(step)));
    }

    /// Bars start where `bar_start` says, one after another, and the bar
    /// holding a bar's start is that bar. Bars are numbered from 1 at tick
    /// zero and from −1 before it, with no bar 0, as in Live.
    #[test]
    fn bars_tile_the_timeline(map in signature_map(), bar in -10_000i64..=100_000) {
        let start = map.bar_start(bar);
        prop_assert_eq!(map.bar_at(start), (bar, Ticks::ZERO));
        prop_assert!(start < map.bar_start(bar + 1));
        let number = if bar >= 0 { bar + 1 } else { bar };
        prop_assert_eq!(map.to_bbs(start), BarBeatSixteenth::new(number, 1, 1));
    }
}

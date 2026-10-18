//! The warp map's arithmetic (the warping spec, R2): where a beat of a clip's
//! contents falls in its file, and the other way round.
//!
//! A map is a list of markers pairing a time in the file with a beat, and the
//! clip's tempo. Between two markers the file's time maps to beats in a
//! straight line; past the last marker the file runs at the clip's tempo, and
//! before the first at the first segment's tempo (the clip's tempo when there
//! is only one marker). Beats are ticks, 705,600 to the quarter note, kept as
//! `i64` in the markers and `f64` in the arithmetic.

/// Ticks to the quarter note, as `bl_core::time::TICKS_PER_QUARTER`: the map
/// works in plain numbers so this crate needs no document types.
const TICKS_PER_QUARTER: f64 = 705_600.0;

/// Ticks a tempo of 1 BPM covers in a second.
const TICKS_PER_SECOND_PER_BPM: f64 = TICKS_PER_QUARTER / 60.0;

/// The tempos a clip may have, as the tempo map accepts them.
pub const MIN_BPM: f64 = 1.0;
/// See [`MIN_BPM`].
pub const MAX_BPM: f64 = 1_000.0;

/// One warp marker: a time in the file and the beat it plays on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    /// Seconds into the file.
    pub time: f64,
    /// Ticks from the clip's 1.1.1: negative before it.
    pub beat: i64,
}

/// A checked warp map.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpMap {
    markers: Vec<Marker>,
    bpm: f64,
}

impl WarpMap {
    /// A map of `markers` and the clip's tempo `bpm`.
    ///
    /// # Errors
    ///
    /// Says what is wrong if there are no markers, a time is negative or not
    /// finite, the markers aren't in increasing order of both time and
    /// beat, or the tempo isn't from 1 to 1,000 BPM.
    pub fn new(markers: Vec<Marker>, bpm: f64) -> Result<Self, &'static str> {
        if markers.is_empty() {
            return Err("a warped clip needs at least one warp marker");
        }
        if markers.iter().any(|m| !m.time.is_finite() || m.time < 0.0) {
            return Err("a warp marker's time in the file is finite and not negative");
        }
        if markers
            .windows(2)
            .any(|pair| pair[1].time <= pair[0].time || pair[1].beat <= pair[0].beat)
        {
            return Err("warp markers are in increasing order of time and of beat");
        }
        if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
            return Err("a clip's tempo is from 1 to 1,000 BPM");
        }
        Ok(Self { markers, bpm })
    }

    /// The markers, in order.
    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The clip's tempo, which the file runs at after the last marker.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    /// The tempo of the segment that starts at marker `index`: the beats it
    /// spans per minute of the file it spans, and for the last marker the
    /// clip's tempo.
    #[must_use]
    pub fn segment_bpm(&self, index: usize) -> f64 {
        match self.markers.get(index..index + 2) {
            Some([a, b]) => segment_bpm(*a, *b),
            _ => self.bpm,
        }
    }

    /// The first segment's tempo, or the clip's when there is one marker:
    /// what the file runs at before the first marker.
    #[must_use]
    pub fn first_bpm(&self) -> f64 {
        self.segment_bpm(0)
    }

    /// The beat, in ticks, that `time` seconds into the file plays on. A NaN
    /// time gives NaN.
    #[must_use]
    pub fn time_to_beat(&self, time: f64) -> f64 {
        let (first, last) = (self.markers[0], self.markers[self.markers.len() - 1]);
        // NaN takes this branch: the search below has no place for it.
        if time.is_nan() || time <= first.time {
            return first.beat as f64
                + (time - first.time) * self.first_bpm() * TICKS_PER_SECOND_PER_BPM;
        }
        if time >= last.time {
            return last.beat as f64 + (time - last.time) * self.bpm * TICKS_PER_SECOND_PER_BPM;
        }
        let i = self.markers.partition_point(|m| m.time <= time) - 1;
        let (a, b) = (self.markers[i], self.markers[i + 1]);
        a.beat as f64 + (time - a.time) * (b.beat - a.beat) as f64 / (b.time - a.time)
    }

    /// The time in the file, in seconds, that plays on `beat` ticks. A NaN
    /// beat gives NaN.
    #[must_use]
    pub fn beat_to_time(&self, beat: f64) -> f64 {
        let (first, last) = (self.markers[0], self.markers[self.markers.len() - 1]);
        // NaN takes this branch: the search below has no place for it.
        if beat.is_nan() || beat <= first.beat as f64 {
            return first.time
                + (beat - first.beat as f64) / (self.first_bpm() * TICKS_PER_SECOND_PER_BPM);
        }
        if beat >= last.beat as f64 {
            return last.time + (beat - last.beat as f64) / (self.bpm * TICKS_PER_SECOND_PER_BPM);
        }
        let i = self.markers.partition_point(|m| m.beat as f64 <= beat) - 1;
        let (a, b) = (self.markers[i], self.markers[i + 1]);
        a.time + (beat - a.beat as f64) * (b.time - a.time) / (b.beat - a.beat) as f64
    }
}

/// The tempo of the straight segment from `a` to `b`.
fn segment_bpm(a: Marker, b: Marker) -> f64 {
    (b.beat - a.beat) as f64 / ((b.time - a.time) * TICKS_PER_SECOND_PER_BPM)
}

/// The ticks a segment of tempo `bpm` spans in `seconds` of the file.
#[must_use]
pub fn ticks_in(bpm: f64, seconds: f64) -> f64 {
    bpm * seconds * TICKS_PER_SECOND_PER_BPM
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn map(markers: &[(f64, i64)], bpm: f64) -> WarpMap {
        let markers = markers
            .iter()
            .map(|&(time, beat)| Marker { time, beat })
            .collect();
        WarpMap::new(markers, bpm).unwrap()
    }

    #[test]
    fn nan_maps_to_nan() {
        let m = map(&[(1.0, 0), (2.0, 705_600), (3.0, 1_411_200)], 60.0);
        assert!(m.time_to_beat(f64::NAN).is_nan());
        assert!(m.beat_to_time(f64::NAN).is_nan());
    }

    // Spec: warping R2
    #[test]
    fn a_map_runs_straight_between_markers_and_at_the_clips_tempo_past_them() {
        // Two quarter notes in a second between the markers: 120 BPM; the
        // clip's own tempo, 60, after the last; 120 again before the first.
        let q = 705_600_i64;
        let qf = q as f64;
        let m = map(&[(1.0, 0), (2.0, 2 * q)], 60.0);
        assert!((m.first_bpm() - 120.0).abs() < 1e-9);
        assert!((m.segment_bpm(0) - 120.0).abs() < 1e-9);
        assert!((m.segment_bpm(1) - 60.0).abs() < 1e-9);
        assert!((m.time_to_beat(1.5) - qf).abs() < 1e-6);
        assert!((m.time_to_beat(3.0) - 3.0 * qf).abs() < 1e-6);
        assert!((m.time_to_beat(0.5) + qf).abs() < 1e-6);
        assert!((m.beat_to_time(0.0) - 1.0).abs() < 1e-12);
        assert!((m.beat_to_time(3.0 * qf) - 3.0).abs() < 1e-9);
        // One marker: the clip's tempo both sides.
        let one = map(&[(2.0, 0)], 90.0);
        assert!((one.first_bpm() - 90.0).abs() < 1e-12);
        assert!((one.beat_to_time(-qf) - (2.0 - 60.0 / 90.0)).abs() < 1e-9);
    }

    #[test]
    fn a_map_must_be_whole() {
        let marker = |time, beat| Marker { time, beat };
        assert!(WarpMap::new(vec![], 120.0).is_err());
        assert!(WarpMap::new(vec![marker(-1.0, 0)], 120.0).is_err());
        assert!(WarpMap::new(vec![marker(f64::NAN, 0)], 120.0).is_err());
        assert!(WarpMap::new(vec![marker(0.0, 0), marker(1.0, 0)], 120.0).is_err());
        assert!(WarpMap::new(vec![marker(0.0, 0), marker(0.0, 1)], 120.0).is_err());
        assert!(WarpMap::new(vec![marker(0.0, 0)], 0.5).is_err());
        assert!(WarpMap::new(vec![marker(0.0, 0)], 1_000.5).is_err());
        assert!(WarpMap::new(vec![marker(0.0, 0)], 1_000.0).is_ok());
    }

    /// Random maps: increasing times and beats, tempos within the allowed range.
    fn arb_map() -> impl Strategy<Value = WarpMap> {
        (
            proptest::collection::vec((0.01_f64..5.0, 1_000_i64..4_000_000), 1..12),
            0.0_f64..3.0,
            -2_000_000_i64..2_000_000,
            1.0_f64..1_000.0,
        )
            .prop_map(|(steps, time0, beat0, bpm)| {
                let (mut time, mut beat) = (time0, beat0);
                let markers = steps
                    .into_iter()
                    .map(|(dt, db)| {
                        time += dt;
                        beat += db;
                        Marker { time, beat }
                    })
                    .collect();
                WarpMap::new(markers, bpm).unwrap()
            })
    }

    proptest! {
        // Spec: warping R2: beat to time and back stays within a tick.
        #[test]
        fn beat_to_time_to_beat_is_within_a_tick(map in arb_map(), beat in -50_000_000_i64..50_000_000) {
            let back = map.time_to_beat(map.beat_to_time(beat as f64));
            prop_assert!((back - beat as f64).abs() < 1.0, "{beat} came back as {back}");
        }

        #[test]
        fn time_to_beat_to_time_is_within_a_microsecond(map in arb_map(), time in 0.0_f64..100.0) {
            let back = map.beat_to_time(map.time_to_beat(time));
            prop_assert!((back - time).abs() < 1e-6, "{time} came back as {back}");
        }

        #[test]
        fn the_map_only_goes_forward(map in arb_map(), a in -1e7_f64..1e7, b in -1e7_f64..1e7) {
            prop_assume!(a < b);
            prop_assert!(map.beat_to_time(a) <= map.beat_to_time(b));
        }
    }
}

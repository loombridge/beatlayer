//! Property tests for the tempo map's conversions (WP-0.5 acceptance).
//!
//! Each property runs 10,000 cases. Tempo maps have up to nine segments,
//! mixing steady stretches, ramps up and down, and jumps between them.

use std::ops::RangeInclusive;
use std::sync::OnceLock;

use bl_core::time::{Bpm, Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks};
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
/// Positions run to a million quarter notes either side of zero: 139 hours
/// at 120 BPM.
const POSITIONS: i64 = 1_000_000 * Q;
/// Frame positions run to 2³⁶ either side of zero: four days at 192 kHz.
const FRAMES: i64 = 1 << 36;
/// One nanosecond.
const NANOSECOND: f64 = 1e-9;

fn bpm_in(range: RangeInclusive<f64>) -> impl Strategy<Value = Bpm> + Clone {
    range.prop_map(|bpm| Bpm::new(bpm).unwrap())
}

/// Every tempo the type allows: 1 to 1,000 BPM.
fn any_bpm() -> impl Strategy<Value = Bpm> + Clone {
    bpm_in(Bpm::MIN.get()..=Bpm::MAX.get())
}

/// Every tempo Live allows: 20 to 999 BPM.
fn live_bpm() -> impl Strategy<Value = Bpm> + Clone {
    bpm_in(20.0..=999.0)
}

/// A map of up to eight constant or ramp segments, each up to
/// `max_quarters` long, then a final constant segment.
fn tempo_map(
    bpm: impl Strategy<Value = Bpm> + Clone,
    max_quarters: i64,
) -> impl Strategy<Value = TempoMap> {
    let length = prop_oneof![1i64..=1_000, 1i64..=Q, 1i64..=max_quarters * Q];
    let segment = (length, any::<bool>(), bpm.clone(), bpm.clone());
    (prop::collection::vec(segment, 0..=8), bpm).prop_map(|(segments, last)| {
        let mut start = Ticks::ZERO;
        let mut built = Vec::with_capacity(segments.len() + 1);
        for (length, ramp, from, to) in segments {
            built.push(if ramp {
                TempoSegment::Ramp { start, from, to }
            } else {
                TempoSegment::Constant { start, bpm: from }
            });
            start += Ticks(length);
        }
        built.push(TempoSegment::Constant { start, bpm: last });
        TempoMap::new(built).unwrap()
    })
}

fn segment_starts(map: &TempoMap) -> Vec<Ticks> {
    map.segments().map(TempoSegment::start).collect()
}

/// A map and a position: a third near a segment boundary, a third inside
/// the segments, and a third anywhere up to `spread` either side of zero.
fn map_and_position(
    map: impl Strategy<Value = TempoMap>,
    spread: i64,
) -> impl Strategy<Value = (TempoMap, Ticks)> {
    (map, any::<Index>(), -64i64..=64, -spread..=spread, 0u8..3).prop_map(
        |(map, index, offset, anywhere, pick)| {
            let starts = segment_starts(&map);
            let end = starts.last().unwrap().get();
            let ticks = match pick {
                0 => starts[index.index(starts.len())] + Ticks(offset),
                1 => Ticks(index.index((end + 2 * Q) as usize) as i64 - Q),
                _ => Ticks(anywhere),
            };
            (map, ticks)
        },
    )
}

/// The common device rates, or any rate from `range`.
fn sample_rate(range: RangeInclusive<u32>) -> impl Strategy<Value = SampleRate> {
    prop_oneof![
        prop::sample::select(vec![
            SampleRate::HZ_44100,
            SampleRate::HZ_48000,
            SampleRate::HZ_88200,
            SampleRate::HZ_96000,
            SampleRate::HZ_176400,
            SampleRate::HZ_192000,
        ]),
        range.prop_map(|hz| SampleRate::new(hz).unwrap()),
    ]
}

/// Slack for `f64` rounding in a time of `seconds`: a few units in the last
/// place.
fn slack(seconds: f64) -> f64 {
    8.0 * f64::EPSILON * seconds.abs().max(1.0)
}

proptest! {
    #![proptest_config(config())]

    /// The acceptance criterion: ticks to frames to ticks lands within one
    /// tick. The frame position here is the exact one, before it's rounded
    /// to a whole frame; the next property covers whole frames.
    #[test]
    fn ticks_to_frames_to_ticks_is_within_one_tick(
        (map, ticks) in map_and_position(tempo_map(any_bpm(), 2_000), POSITIONS),
        rate in sample_rate(8_000..=384_000),
    ) {
        let hz = rate.as_f64();
        let frames = map.ticks_to_seconds(ticks) * hz;
        let back = map.seconds_to_ticks(frames / hz);
        prop_assert!(
            (back - ticks).abs() <= Ticks(1),
            "{ticks} -> {frames} frames -> {back}"
        );
    }

    /// Through whole [`Frames`]: the frame is the nearest one to the tick,
    /// and the tick that comes back is the nearest one to that frame. A
    /// frame spans many ticks at most tempos, so this is as close as whole
    /// frames allow: within half a frame plus half a tick.
    #[test]
    fn ticks_to_whole_frames_to_ticks_is_nearest_each_way(
        (map, ticks) in map_and_position(tempo_map(any_bpm(), 2_000), POSITIONS),
        rate in sample_rate(8_000..=384_000),
    ) {
        let hz = rate.as_f64();
        let seconds = map.ticks_to_seconds(ticks);
        let frames = map.ticks_to_frames(ticks, rate);
        let frame_seconds = rate.frames_to_seconds(frames);
        prop_assert!(
            (frame_seconds - seconds).abs() <= 0.5 / hz + slack(seconds),
            "{ticks} -> {frames} isn't the nearest frame"
        );

        let back = map.frames_to_ticks(frames, rate);
        let distance = |t: Ticks| (map.ticks_to_seconds(t) - frame_seconds).abs();
        let here = distance(back);
        prop_assert!(here <= distance(back - Ticks(1)) + slack(seconds));
        prop_assert!(here <= distance(back + Ticks(1)) + slack(seconds));
    }

    /// Whole frames to ticks to frames comes back to the same frame at every
    /// tempo Live allows (20 to 999 BPM) and every rate up to 192 kHz,
    /// because a tick is then shorter than a frame.
    #[test]
    fn frames_to_ticks_to_frames_is_exact(
        map in tempo_map(live_bpm(), 2_000),
        rate in sample_rate(8_000..=192_000),
        anywhere in -FRAMES..=FRAMES,
        index in any::<Index>(),
        offset in -64i64..=64,
        near_boundary in any::<bool>(),
    ) {
        let frames = if near_boundary {
            let starts = segment_starts(&map);
            map.ticks_to_frames(starts[index.index(starts.len())], rate) + Frames(offset)
        } else {
            Frames(anywhere)
        };
        let ticks = map.frames_to_ticks(frames, rate);
        prop_assert_eq!(map.ticks_to_frames(ticks, rate), frames, "via {}", ticks);
    }

    /// Ticks to seconds to ticks is exact.
    #[test]
    fn ticks_to_seconds_to_ticks_is_exact(
        (map, ticks) in map_and_position(tempo_map(any_bpm(), 2_000), POSITIONS),
    ) {
        let seconds = map.ticks_to_seconds(ticks);
        prop_assert_eq!(map.seconds_to_ticks(seconds), ticks, "via {} s", seconds);
    }

    /// Later ticks are never earlier in seconds or frames, and later frames
    /// are never earlier in ticks, across segment boundaries too.
    #[test]
    fn conversions_are_monotonic(
        (map, ticks) in map_and_position(tempo_map(any_bpm(), 2_000), POSITIONS),
        step in prop_oneof![1i64..=2, 1i64..=1_000, 1i64..=Q],
        rate in sample_rate(8_000..=384_000),
    ) {
        let later = ticks + Ticks(step);
        prop_assert!(map.ticks_to_seconds(ticks) < map.ticks_to_seconds(later));
        prop_assert!(map.ticks_to_frames(ticks, rate) <= map.ticks_to_frames(later, rate));

        let frames = map.ticks_to_frames(ticks, rate);
        let later_frames = frames + Frames(step);
        prop_assert!(map.frames_to_ticks(frames, rate) <= map.frames_to_ticks(later_frames, rate));
    }

    /// The acceptance criterion: a ramp's closed-form time matches numerical
    /// integration of its tempo curve to within 1 ns, at every point of
    /// ramps up to 1,000 quarter notes long (16.7 hours at 1 BPM) between
    /// any two tempos the type allows.
    #[test]
    fn ramp_matches_numerical_integration_within_1_ns(
        from in any_bpm(),
        to in any_bpm(),
        length in prop_oneof![1i64..=Q, 1i64..=1_000 * Q],
        index in any::<Index>(),
    ) {
        let end = Ticks(length);
        let map = TempoMap::new([
            TempoSegment::Ramp { start: Ticks::ZERO, from, to },
            TempoSegment::Constant { start: end, bpm: to },
        ]).unwrap();
        let at = index.index(length as usize + 1) as i64;
        let numerical = numeric::ramp_seconds(from.get(), to.get(), length, at);
        let closed = map.ticks_to_seconds(Ticks(at));
        prop_assert!(
            (closed - numerical).abs() <= NANOSECOND,
            "closed form {closed} s, numerical {numerical} s, {} ns apart",
            (closed - numerical).abs() * 1e9
        );
    }

    /// The same for whole maps: every segment up to a position, at Live's
    /// tempos, with up to 16,000 quarter notes of segments (13.3 hours at
    /// 20 BPM).
    #[test]
    fn map_matches_numerical_integration_within_1_ns(
        (map, ticks) in map_and_position(tempo_map(live_bpm(), 2_000), 16_000 * Q),
    ) {
        // Integrate from zero, so take positions at or after it.
        let ticks = ticks.abs();
        let starts = segment_starts(&map);
        let mut total = numeric::Sum::default();
        for (index, segment) in map.segments().enumerate() {
            let start = segment.start();
            if start >= ticks {
                break;
            }
            let end = starts.get(index + 1).copied().unwrap_or(Ticks::MAX).min(ticks);
            let length = starts.get(index + 1).map_or(i64::MAX, |next| (*next - start).get());
            let (from, to) = (segment.start_bpm().get(), segment.end_bpm().get());
            total.add(numeric::ramp_seconds(from, to, length, (end - start).get()));
        }
        let numerical = total.total();
        let closed = map.ticks_to_seconds(ticks);
        prop_assert!(
            (closed - numerical).abs() <= NANOSECOND,
            "closed form {closed} s, numerical {numerical} s, {} ns apart",
            (closed - numerical).abs() * 1e9
        );
    }
}

/// An independent numerical integrator for the tempo curve: composite
/// Gauss–Legendre quadrature with compensated summation. It shares nothing
/// with the closed form but the definition of the curve.
mod numeric {
    use super::*;

    /// Nodes per panel. The panels are graded so that each spans a tempo
    /// ratio of at most 2^(1/4), where ten nodes are accurate far below
    /// `f64` precision.
    const ORDER: usize = 10;

    /// Seconds from the start of a segment to `at` ticks into it, where the
    /// tempo runs in a straight line from `from` BPM at the start to `to`
    /// BPM at `length` ticks.
    pub fn ramp_seconds(from: f64, to: f64, length: i64, at: i64) -> f64 {
        let length = length as f64;
        let bpm = |x: f64| {
            if from == to {
                from
            } else {
                from + (to - from) * (x / length)
            }
        };
        let seconds_per_tick = |x: f64| 60.0 / (bpm(x) * TICKS_PER_QUARTER as f64);
        let end = at as f64;
        integrate(
            seconds_per_tick,
            &panel_edges(from, bpm(end), to, length, end),
        )
    }

    /// Panel edges from 0 to `end`, spaced evenly in log tempo, so each
    /// panel spans the same tempo ratio and none sits too close to the
    /// curve's pole.
    fn panel_edges(from: f64, end_bpm: f64, to: f64, length: f64, end: f64) -> Vec<f64> {
        let ratio = (end_bpm / from).max(from / end_bpm);
        if ratio < 1.0001 {
            return vec![0.0, 0.5 * end, end];
        }
        let panels = 2 + (4.0 * ratio.log2()).ceil() as usize;
        let mut edges = vec![0.0];
        for j in 1..panels {
            let bpm = from * (end_bpm / from).powf(j as f64 / panels as f64);
            edges.push((length * (bpm - from) / (to - from)).clamp(0.0, end));
        }
        edges.push(end);
        edges
    }

    /// The integral of `f` over the panels between consecutive `edges`.
    /// Adjacent panels share an edge, so they tile the range exactly.
    pub fn integrate(f: impl Fn(f64) -> f64, edges: &[f64]) -> f64 {
        let mut sum = Sum::default();
        for panel in edges.windows(2) {
            let half = 0.5 * (panel[1] - panel[0]);
            let middle = 0.5 * (panel[0] + panel[1]);
            for &(node, weight) in rule() {
                sum.add(weight * half * f(middle + half * node));
            }
        }
        sum.total()
    }

    fn rule() -> &'static [(f64, f64)] {
        static RULE: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
        RULE.get_or_init(|| gauss_legendre(ORDER))
    }

    /// Gauss–Legendre nodes and weights on [−1, 1], by Newton's method on
    /// the Legendre polynomial.
    fn gauss_legendre(n: usize) -> Vec<(f64, f64)> {
        (0..n)
            .map(|i| {
                let mut x = (std::f64::consts::PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
                for _ in 0..100 {
                    let (p, dp) = legendre(n, x);
                    let step = p / dp;
                    x -= step;
                    if step.abs() < 1e-16 {
                        break;
                    }
                }
                let (_, dp) = legendre(n, x);
                (x, 2.0 / ((1.0 - x * x) * dp * dp))
            })
            .collect()
    }

    /// `P_n(x)` and its derivative.
    fn legendre(n: usize, x: f64) -> (f64, f64) {
        let (mut before, mut p) = (1.0, x);
        for k in 2..=n {
            let k = k as f64;
            let next = ((2.0 * k - 1.0) * x * p - (k - 1.0) * before) / k;
            before = p;
            p = next;
        }
        (p, n as f64 * (x * p - before) / (x * x - 1.0))
    }

    /// Neumaier's compensated sum.
    #[derive(Default)]
    pub struct Sum {
        sum: f64,
        compensation: f64,
    }

    impl Sum {
        pub fn add(&mut self, value: f64) {
            let sum = self.sum + value;
            self.compensation += if self.sum.abs() >= value.abs() {
                (self.sum - sum) + value
            } else {
                (value - sum) + self.sum
            };
            self.sum = sum;
        }

        pub fn total(&self) -> f64 {
            self.sum + self.compensation
        }
    }
}

/// The integrator is itself checked against integrals known in closed form,
/// so a pass above means the tempo map is right, not that both are wrong in
/// the same way.
#[test]
fn the_numerical_integrator_is_accurate() {
    // Polynomials up to degree 2n − 1 are exact.
    let x18 = numeric::integrate(|x| x.powi(18), &[-1.0, 1.0]);
    assert!((x18 - 2.0 / 19.0).abs() < 1e-15, "{x18}");

    // 1 / (1 + 999x) on [0, 1] has a pole just left of the range.
    let edges: Vec<f64> = (0..=40)
        .map(|j| (1000f64.powf(j as f64 / 40.0) - 1.0) / 999.0)
        .collect();
    let hard = numeric::integrate(|x| 1.0 / (1.0 + 999.0 * x), &edges);
    let exact = 1000f64.ln() / 999.0;
    assert!((hard - exact).abs() < 1e-16, "{hard} vs {exact}");

    // A steady 120 BPM is half a second per quarter note.
    let steady = numeric::ramp_seconds(120.0, 120.0, 10 * Q, 10 * Q);
    assert!((steady - 5.0).abs() < 1e-14, "{steady}");

    // 60 to 120 BPM over 8 quarter notes takes 8 ln 2 seconds.
    let ramp = numeric::ramp_seconds(60.0, 120.0, 8 * Q, 8 * Q);
    assert!((ramp - 8.0 * 2f64.ln()).abs() < 1e-14, "{ramp}");
}

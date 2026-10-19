//! Auto-warp in the document (WP-4.9, the warping spec R32 to R35 and R38
//! to R40): the warp maps a loop or a long sample gets from its file's
//! beats, Set 1.1.1 Here, Warp From Here, and Warp as Loop. Like [`warp`],
//! every edit takes the clip's data and returns the edited data or says why
//! it can't; the document holds no media, so the file's analysis comes in
//! as [`FileBeats`], which the app fills in.
//!
//! [`warp`]: crate::warp

use bl_core::time::Ticks;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::AudioData;
use crate::error::ModelError;
use crate::warp::{MarkerAt, WarpMarker};
use crate::warp_settings::{WarpMode, WarpSettings};

/// The bar counts a short sample warped as a loop may be (the warping spec,
/// R34).
pub const EVEN_LOOP_BARS: [u32; 5] = [1, 2, 4, 8, 16];

/// The bar counts Warp as x-Bar Loop offers (R40).
pub const LOOP_BARS: [u32; 6] = [1, 2, 4, 8, 16, 32];

/// The fastest and slowest tempo a file's beats are doubled or halved to
/// reach a starting tempo (the analysis's own range).
const BEATS_BPM: std::ops::RangeInclusive<f64> = 40.0..=240.0;

/// What a file's analysis gives auto-warp (the warping spec, R30).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A file's tempo and beats from its analysis, as `clip.transients` reports \
                   readiness: `seconds` is the file's length, `bpm` its tempo, `beats` the times \
                   in seconds of its beats, in order."
)]
pub struct FileBeats {
    /// The file's length in seconds.
    pub seconds: f64,
    /// Its tempo in BPM, or `null` if it has no rhythm to measure.
    pub bpm: Option<f64>,
    /// The times in seconds of its beats, in increasing order.
    pub beats: Vec<f64>,
}

/// What Save Default Clip keeps of an audio clip (the warping spec, R42): the
/// settings a new clip of the same file starts with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An audio clip's saved defaults: its Warp switch, warp markers and tempo, warp \
                   mode and the mode controls (which hold its Transpose and Detune), and gain. \
                   Times in the markers are as in `data.audio.warp_markers`."
)]
pub struct ClipDefaults {
    /// The Warp switch.
    pub warp: bool,
    /// The warp markers.
    pub warp_markers: Vec<WarpMarker>,
    /// The clip's tempo in BPM; given whenever there are markers.
    pub tempo: Option<f64>,
    /// The warp mode.
    pub warp_mode: WarpMode,
    /// Every mode's controls.
    pub warp_settings: WarpSettings,
    /// The gain, as a linear gain.
    pub gain: f64,
}

/// Where in a warped clip a command acts: give one of the three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WarpPoint {
    /// The selected warp marker, by its place in the map, the first being 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<usize>,
    /// The Sample Editor's insert marker as a beat, in ticks from the clip's
    /// 1.1.1: a warp marker is made there, at the time the map already
    /// places it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<Ticks>,
    /// The insert marker as a time in the file, in seconds: a warp marker is
    /// made there, on the beat the map already places it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
}

/// How Warp From Here warps what follows its point (the warping spec, R39).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarpFrom {
    /// Warp From Here: a marker on every bar after it, from the tempo found
    /// there.
    Auto,
    /// Warp From Here (Start At …): the same, starting from the tempo `bpm`
    /// (the set's tempo there unless given).
    StartAt,
    /// Warp From Here (Straight): no markers after it, the clip's tempo the
    /// tempo found after it.
    Straight,
    /// Warp … BPM From Here: no markers after it, the clip's tempo `bpm`
    /// (the set's tempo there unless given).
    Bpm,
}

/// Which edge of a clip a stretch drag holds (the warping spec, R47): the
/// dragged edge moves and the other stays put, and every beat of the clip's
/// contents scales about the beat at the fixed edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipEdge {
    /// The left edge, so the right edge stays put.
    Left,
    /// The right edge, so the left edge stays put.
    Right,
}

/// The bar count of `counts` whose tempo, for a file of `seconds` taken as
/// that many bars of 4/4, is nearest `bpm`.
#[must_use]
pub fn nearest_bars(counts: &[u32], seconds: f64, bpm: f64) -> u32 {
    let distance = |bars: u32| (loop_bpm(seconds, bars) - bpm).abs();
    counts
        .iter()
        .copied()
        .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
        .expect("there is always a count to choose")
}

/// The tempo of `seconds` of audio taken as `bars` bars of 4/4.
#[must_use]
pub fn loop_bpm(seconds: f64, bars: u32) -> f64 {
    240.0 * f64::from(bars) / seconds
}

/// A marker on the first beat of each bar, counting four beats from
/// `beats[first]`, which sits on `beat`.
fn bar_markers(beats: &[f64], first: usize, beat: Ticks) -> Vec<WarpMarker> {
    (first..beats.len())
        .step_by(4)
        .zip(0..)
        .map(|(i, bar)| WarpMarker {
            time: beats[i],
            beat: beat + Ticks::WHOLE * bar,
        })
        .collect()
}

/// The tempo of the last bar of `markers`, or `fallback` for a single one.
fn last_bar_bpm(markers: &[WarpMarker], fallback: Option<f64>) -> Result<f64, ModelError> {
    match markers {
        [.., before, last] => Ok(240.0 / (last.time - before.time)),
        _ => fallback.ok_or(ModelError::InvalidInput("the file has no tempo to warp to")),
    }
}

/// `file`'s beats, doubled or halved until their tempo is nearest `hint`,
/// and where `nearest`, an index into them, falls among the result.
fn beats_near(file: &FileBeats, hint: f64, nearest: usize) -> (Vec<f64>, Option<f64>, usize) {
    let (mut beats, mut bpm, mut at) = (file.beats.clone(), file.bpm, nearest);
    while let Some(now) = bpm.filter(|&now| now * 1.5 < hint && now * 2.0 <= *BEATS_BPM.end()) {
        let mut doubled = Vec::with_capacity(beats.len() * 2);
        for pair in beats.windows(2) {
            doubled.extend([pair[0], (pair[0] + pair[1]) / 2.0]);
        }
        if let [.., before, last] = beats[..] {
            let half = last + (last - before) / 2.0;
            doubled.extend(
                [last, half]
                    .into_iter()
                    .take(if half < file.seconds { 2 } else { 1 }),
            );
        } else {
            doubled.clone_from(&beats);
        }
        (beats, bpm, at) = (doubled, Some(now * 2.0), at * 2);
    }
    while let Some(now) = bpm.filter(|&now| now > hint * 1.5 && now / 2.0 >= *BEATS_BPM.start()) {
        let parity = at % 2;
        beats = beats.into_iter().skip(parity).step_by(2).collect();
        (bpm, at) = (Some(now / 2.0), at / 2);
    }
    (beats, bpm, at)
}

impl AudioData {
    /// The settings Save Default Clip keeps (R42).
    #[must_use]
    pub fn defaults(&self) -> ClipDefaults {
        ClipDefaults {
            warp: self.warp,
            warp_markers: self.warp_markers.clone(),
            tempo: self.tempo,
            warp_mode: self.warp_mode,
            warp_settings: self.warp_settings,
            gain: self.gain,
        }
    }

    /// The clip with `defaults` in place of its own settings: what a new clip
    /// of a file with saved defaults is (R33). Its Start and End markers and
    /// loop brace stay at the file's ends.
    ///
    /// # Errors
    ///
    /// If the markers and tempo don't make a map, or the gain isn't finite
    /// and 0 or more.
    pub fn with_defaults(self, defaults: &ClipDefaults) -> Result<Self, ModelError> {
        if !defaults.gain.is_finite() || defaults.gain < 0.0 {
            return Err(ModelError::InvalidInput("a gain is finite and 0 or more"));
        }
        let next = Self {
            warp: defaults.warp,
            warp_markers: defaults.warp_markers.clone(),
            tempo: defaults.tempo,
            warp_mode: defaults.warp_mode,
            warp_settings: defaults.warp_settings,
            gain: defaults.gain,
            ..self
        };
        if next.warp || !next.warp_markers.is_empty() {
            next.warp_map()?;
        }
        Ok(next)
    }

    /// The clip warped as a loop of `bars` bars of 4/4 over the part of its
    /// file from `from` to `to` seconds (R34, R40): a marker at each end,
    /// on 1.1.1 and on the last bar's end, the loop brace and the Start and
    /// End markers on it, the loop `looping`, and that tempo as the clip's.
    ///
    /// # Errors
    ///
    /// If the part is empty or its tempo is out of the map's range.
    pub fn warped_as_loop(
        &self,
        from: f64,
        to: f64,
        bars: u32,
        looping: bool,
    ) -> Result<Self, ModelError> {
        if !(from.is_finite() && from >= 0.0 && to > from && bars > 0) {
            return Err(ModelError::InvalidInput(
                "a loop needs a part of the file, and at least one bar",
            ));
        }
        let end = Ticks::WHOLE * i64::from(bars);
        let next = Self {
            warp: true,
            looping,
            warp_markers: vec![
                WarpMarker {
                    time: from,
                    beat: Ticks::ZERO,
                },
                WarpMarker {
                    time: to,
                    beat: end,
                },
            ],
            tempo: Some(loop_bpm(to - from, bars)),
            start: 0.0,
            end: Some(end.get() as f64),
            loop_start: 0.0,
            loop_end: Some(end.get() as f64),
            ..self.clone()
        };
        next.warp_map()?;
        Ok(next)
    }

    /// The clip warped by `markers` and `tempo`, which pass its Start and
    /// End markers and loop brace, seconds in the file, through the new map
    /// (R37): they become the ticks of its contents, to the nearest tick.
    ///
    /// # Errors
    ///
    /// If the markers and tempo don't make a map.
    pub fn warped_by(&self, markers: Vec<WarpMarker>, tempo: f64) -> Result<Self, ModelError> {
        let next = Self {
            warp_markers: markers,
            tempo: Some(tempo),
            ..self.clone()
        };
        let map = next.warp_map()?;
        Ok(Self {
            warp: true,
            ..next.converted(|seconds| map.time_to_beat(seconds), true)
        })
    }

    /// The clip auto-warped (R35): a marker on the first beat of every bar
    /// of `file`'s beats, counting bars of 4/4 from the first beat, which is
    /// 1.1.1 and where the Start marker goes; the End marker at the file's
    /// end, the loop off, and the last bar's tempo as the clip's.
    ///
    /// # Errors
    ///
    /// If `file` has no beats or no tempo, or its tempo is out of range.
    pub fn auto_warped(&self, file: &FileBeats) -> Result<Self, ModelError> {
        let markers = bar_markers(&file.beats, 0, Ticks::ZERO);
        if markers.is_empty() {
            return Err(ModelError::InvalidInput("the file has no beats to warp to"));
        }
        let tempo = last_bar_bpm(&markers, file.bpm)?;
        let next = Self {
            warp: true,
            looping: false,
            warp_markers: markers,
            tempo: Some(tempo),
            start: 0.0,
            end: None,
            loop_start: 0.0,
            loop_end: None,
            ..self.clone()
        };
        next.warp_map()?;
        Ok(next)
    }

    /// The clip with a warp marker at `at`, made if there was none, and the
    /// marker's place in the map.
    fn with_marker_at(&self, at: &WarpPoint) -> Result<(Self, usize), ModelError> {
        self.require_warp()?;
        let place = match (at.marker, at.beat, at.time) {
            (Some(marker), None, None) => {
                self.check_indices(&[marker])?;
                return Ok((self.clone(), marker));
            }
            (None, Some(beat), None) => MarkerAt::Beat(beat),
            (None, None, Some(time)) => MarkerAt::Time(time),
            _ => {
                return Err(ModelError::InvalidInput(
                    "give the `marker`, the `beat` or the `time` to act at, one of the three",
                ));
            }
        };
        let beat = match place {
            MarkerAt::Beat(beat) => beat,
            MarkerAt::Time(time) => Ticks(self.warp_map()?.time_to_beat(time).round() as i64),
        };
        let with = self.with_marker_inserted(place)?;
        let index = with
            .warp_markers
            .iter()
            .position(|marker| marker.beat == beat)
            .ok_or(ModelError::InvalidInput(
                "no warp marker could be made there",
            ))?;
        Ok((with, index))
    }

    /// Set 1.1.1 Here (R38): a marker at `at`, and every marker, the End
    /// marker and the loop brace moved by the ticks that put it on 1.1.1,
    /// so no segment changes tempo; the Start marker goes to 1.1.1.
    ///
    /// # Errors
    ///
    /// If the clip isn't warped, `at` names nothing, or the End marker or
    /// the loop brace would move before 1.1.1.
    pub(crate) fn with_1_1_1_here(&self, at: &WarpPoint) -> Result<Self, ModelError> {
        let (with, index) = self.with_marker_at(at)?;
        let shift = with.warp_markers[index].beat;
        let moved = |ticks: f64| ticks - shift.get() as f64;
        let markers = with
            .warp_markers
            .iter()
            .map(|marker| {
                Ok(WarpMarker {
                    time: marker.time,
                    beat: marker
                        .beat
                        .checked_sub(shift)
                        .ok_or(ModelError::PositionOverflow)?,
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;
        Ok(Self {
            warp_markers: markers,
            start: 0.0,
            end: with.end.map(moved),
            loop_start: moved(with.loop_start),
            loop_end: with.loop_end.map(moved),
            ..with
        })
    }

    /// Warp From Here (R39): the markers up to and including the one at
    /// `at` stay, and those after it are replaced as `method` says. `file`
    /// is the file's beats and `set_bpm` the set's tempo at the point, which
    /// `bpm` overrides.
    ///
    /// # Errors
    ///
    /// If the clip isn't warped, `at` names nothing, `file` is needed and
    /// missing or has no beats, or the tempo is out of range.
    pub(crate) fn with_warp_from_here(
        &self,
        at: &WarpPoint,
        method: WarpFrom,
        bpm: Option<f64>,
        set_bpm: f64,
        file: Option<&FileBeats>,
    ) -> Result<Self, ModelError> {
        let (with, index) = self.with_marker_at(at)?;
        let here = with.warp_markers[index];
        let mut markers = with.warp_markers[..=index].to_vec();
        let hint = bpm.unwrap_or(set_bpm);
        if method == WarpFrom::Bpm {
            return with.with_map(markers, hint);
        }
        let file = file.ok_or(ModelError::InvalidInput(
            "this needs the file's beats: `analysis`",
        ))?;
        let nearest = file
            .beats
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - here.time).abs().total_cmp(&(b.1 - here.time).abs()))
            .map(|(i, _)| i)
            .ok_or(ModelError::InvalidInput("the file has no beats to warp to"))?;
        let tempo = if method == WarpFrom::Straight {
            let after = &file.beats[nearest..];
            match after {
                [first, .., last] => 60.0 * (after.len() - 1) as f64 / (last - first),
                _ => file
                    .bpm
                    .ok_or(ModelError::InvalidInput("the file has no tempo to warp to"))?,
            }
        } else {
            let (beats, tempo, nearest) = if method == WarpFrom::StartAt {
                beats_near(file, hint, nearest)
            } else {
                (file.beats.clone(), file.bpm, nearest)
            };
            markers.extend(
                bar_markers(&beats, nearest + 4, here.beat + Ticks::WHOLE)
                    .into_iter()
                    .filter(|marker| marker.time > here.time),
            );
            last_bar_bpm(&markers[index..], tempo)?
        };
        with.with_map(markers, tempo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentHash;

    const Q: i64 = 705_600;

    fn clip() -> AudioData {
        AudioData::new(ContentHash([1; 32]))
    }

    /// Beats every half second from 0.25: 120 BPM, in a file 20 s long.
    fn file() -> FileBeats {
        FileBeats {
            seconds: 20.0,
            bpm: Some(120.0),
            beats: (0..39).map(|n| 0.25 + 0.5 * f64::from(n)).collect(),
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // Spec: warping R34
    #[test]
    fn the_bar_count_nearest_the_tempo_is_chosen() {
        // 5.33 s: 2 bars are 90 BPM, 1 bar 45 and 4 bars 180.
        assert_eq!(nearest_bars(&EVEN_LOOP_BARS, 16.0 / 3.0, 90.0), 2);
        assert_eq!(nearest_bars(&EVEN_LOOP_BARS, 16.0 / 3.0, 120.0), 2);
        assert_eq!(nearest_bars(&EVEN_LOOP_BARS, 16.0 / 3.0, 170.0), 4);
        assert_eq!(nearest_bars(&LOOP_BARS, 64.0, 120.0), 32);
    }

    // Spec: warping R34, R40
    #[test]
    fn a_loop_has_a_marker_at_each_end_and_its_brace_on_the_file() {
        let looped = clip().warped_as_loop(0.0, 4.0, 2, true).unwrap();
        assert!(looped.warp && looped.looping);
        assert_eq!(looped.warp_markers[0].beat, Ticks::ZERO);
        assert_eq!(looped.warp_markers[1].beat.get(), 8 * Q);
        assert!(near(looped.tempo.unwrap(), 120.0));
        assert_eq!(
            (looped.start, looped.end, looped.loop_start, looped.loop_end),
            (0.0, Some((8 * Q) as f64), 0.0, Some((8 * Q) as f64))
        );
        let selection = clip().warped_as_loop(1.0, 3.0, 1, true).unwrap();
        assert_eq!(selection.warp_markers[0].time, 1.0);
        assert!(clip().warped_as_loop(2.0, 2.0, 1, true).is_err());
    }

    // Spec: warping R35
    #[test]
    fn auto_warping_puts_a_marker_on_each_bars_first_beat() {
        let warped = clip().auto_warped(&file()).unwrap();
        assert_eq!(warped.warp_markers.len(), 10, "39 beats make 10 bars");
        for (bar, marker) in warped.warp_markers.iter().enumerate() {
            assert!(near(marker.time, 0.25 + 2.0 * bar as f64));
            assert_eq!(marker.beat.get(), 4 * Q * bar as i64);
        }
        assert!(near(warped.tempo.unwrap(), 120.0));
        assert!(warped.warp && !warped.looping && warped.end.is_none());
        let none = FileBeats {
            beats: vec![],
            ..file()
        };
        assert!(clip().auto_warped(&none).is_err());
    }

    // Spec: warping R38
    #[test]
    fn set_1_1_1_here_moves_everything_by_the_same_ticks() {
        let mut warped = clip().auto_warped(&file()).unwrap();
        warped.end = Some((20 * Q) as f64);
        warped.start = (4 * Q) as f64;
        let moved = warped
            .with_1_1_1_here(&WarpPoint {
                marker: Some(2),
                ..WarpPoint::default()
            })
            .unwrap();
        // Bar 3 (8 quarters in) is now 1.1.1.
        assert_eq!(moved.warp_markers[2].beat, Ticks::ZERO);
        assert_eq!(moved.warp_markers[0].beat.get(), -8 * Q);
        assert_eq!(moved.start, 0.0);
        assert_eq!(moved.end, Some((12 * Q) as f64));
        assert_eq!(moved.tempo, warped.tempo);
        // Off the markers, one is made on the way.
        let made = warped
            .with_1_1_1_here(&WarpPoint {
                time: Some(1.25),
                ..WarpPoint::default()
            })
            .unwrap();
        assert_eq!(made.warp_markers.len(), 11);
        assert!(
            made.warp_markers
                .iter()
                .any(|m| near(m.time, 1.25) && m.beat == Ticks::ZERO)
        );
        // Not warped, or an End marker left before 1.1.1.
        assert!(
            clip()
                .with_1_1_1_here(&WarpPoint {
                    marker: Some(0),
                    ..WarpPoint::default()
                })
                .is_err()
        );
        assert!(
            warped
                .with_1_1_1_here(&WarpPoint {
                    marker: Some(9),
                    ..WarpPoint::default()
                })
                .and_then(|moved| crate::command::check_marker_order(&moved))
                .is_err()
        );
    }

    // Spec: warping R39
    #[test]
    fn each_warp_from_here_keeps_the_markers_before_it() {
        let mut warped = clip().auto_warped(&file()).unwrap();
        // Bars 5 on are slower, at 100 BPM.
        warped.warp_markers.truncate(5);
        let at = WarpPoint {
            marker: Some(2),
            ..WarpPoint::default()
        };
        let keep = |audio: &AudioData| audio.warp_markers[..3] == warped.warp_markers[..3];
        let auto = warped
            .with_warp_from_here(&at, WarpFrom::Auto, None, 90.0, Some(&file()))
            .unwrap();
        assert!(keep(&auto) && auto.warp_markers.len() == 10);
        assert!(near(auto.tempo.unwrap(), 120.0));
        let straight = warped
            .with_warp_from_here(&at, WarpFrom::Straight, None, 90.0, Some(&file()))
            .unwrap();
        assert!(keep(&straight) && straight.warp_markers.len() == 3);
        assert!((straight.tempo.unwrap() - 120.0).abs() < 1e-6);
        let fixed = warped
            .with_warp_from_here(&at, WarpFrom::Bpm, None, 90.0, None)
            .unwrap();
        assert!(keep(&fixed) && fixed.warp_markers.len() == 3);
        assert_eq!(fixed.tempo, Some(90.0));
        let given = warped
            .with_warp_from_here(&at, WarpFrom::Bpm, Some(100.0), 90.0, None)
            .unwrap();
        assert_eq!(given.tempo, Some(100.0));
        // Start At the set's 60 BPM halves 120: a marker every other bar.
        let half = warped
            .with_warp_from_here(&at, WarpFrom::StartAt, None, 60.0, Some(&file()))
            .unwrap();
        assert!(keep(&half) && half.warp_markers.len() == 3 + 3);
        assert!(near(half.tempo.unwrap(), 60.0));
        // No analysis, no auto-warp.
        assert!(
            warped
                .with_warp_from_here(&at, WarpFrom::Auto, None, 90.0, None)
                .is_err()
        );
    }
}

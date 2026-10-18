//! The warp map in the document (WP-4.1, the warping spec, R1 to R11).
//!
//! A warped audio clip keeps its warp markers and tempo in its
//! [`AudioData`]; its Start and End markers and loop brace are then whole
//! ticks of its contents, not seconds in the file. The arithmetic of the map
//! is `bl-stretch`'s; this module edits the document's side of it. Every edit
//! here takes the clip's data and returns the edited data, or says why it
//! can't: the commands apply it, and a refused edit changes nothing.

use std::borrow::Cow;

use bl_core::time::{Bpm, Ticks};
use bl_stretch::{MAX_BPM, MIN_BPM, Marker, WarpMap, ticks_in};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip::{AudioData, MARKER_SLACK};
use crate::error::ModelError;

/// One warp marker: a time in the clip's file and the beat it plays on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A warp marker: `time`, seconds into the audio file, plays on `beat`, in ticks \
                   (705,600 to the quarter note) counted from the clip's 1.1.1 (negative before it)."
)]
pub struct WarpMarker {
    /// Seconds into the file: 0 or more.
    #[schemars(range(min = 0.0))]
    pub time: f64,
    /// The beat the time plays on, in ticks from the clip's 1.1.1.
    pub beat: Ticks,
}

/// Where a new warp marker goes (the warping spec, R6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerAt {
    /// At a beat of the clip's contents, at the time in the file the map
    /// already places there.
    Beat(Ticks),
    /// At a time in the file, on the beat the map already places it.
    Time(f64),
}

impl AudioData {
    /// The clip's warp map: its markers and tempo.
    ///
    /// # Errors
    ///
    /// If there are no markers or no tempo, or they break the map's rules
    /// (increasing order, a tempo from 1 to 1,000 BPM).
    pub fn warp_map(&self) -> Result<WarpMap, ModelError> {
        let tempo = self.tempo.ok_or(ModelError::InvalidInput(
            "a clip with warp markers needs a tempo",
        ))?;
        let markers = self
            .warp_markers
            .iter()
            .map(|m| Marker {
                time: m.time,
                beat: m.beat.get(),
            })
            .collect();
        WarpMap::new(markers, tempo).map_err(ModelError::InvalidInput)
    }

    /// The beat of a warped clip's contents the start of its file plays on,
    /// in ticks and perhaps fractional; `None` for an unwarped clip.
    #[must_use]
    pub fn file_start_beat(&self) -> Option<f64> {
        if !self.warp {
            return None;
        }
        self.warp_map().ok().map(|map| map.time_to_beat(0.0))
    }

    /// How many seconds before the start of its file the Start marker is, if
    /// it is: what trimming a clip's left edge too far leaves.
    #[must_use]
    pub(crate) fn seconds_before_file(&self) -> Option<f64> {
        let time = if self.warp {
            self.warp_map().ok()?.beat_to_time(self.start)
        } else {
            self.start
        };
        (time < -MARKER_SLACK).then_some(-time)
    }

    /// The clip's Start and End markers and loop brace in seconds in the
    /// file: what the engine plays until warped playback (WP-4.2) follows the
    /// map. An unwarped clip's are its own.
    #[must_use]
    pub fn in_seconds(&self) -> Cow<'_, Self> {
        let Some(map) = self.warp.then(|| self.warp_map().ok()).flatten() else {
            return Cow::Borrowed(self);
        };
        Cow::Owned(self.converted(|beat| map.beat_to_time(beat).max(0.0), false))
    }

    /// The data with its Start and End markers and loop brace passed through
    /// `convert`, rounded to whole ticks if `whole`, and `warp` left as it is.
    fn converted(&self, convert: impl Fn(f64) -> f64, whole: bool) -> Self {
        let convert = |value: f64| {
            let converted = convert(value);
            if whole { converted.round() } else { converted }
        };
        Self {
            start: convert(self.start),
            end: self.end.map(convert),
            loop_start: convert(self.loop_start),
            loop_end: self.loop_end.map(convert),
            ..self.clone()
        }
    }

    fn require_warp(&self) -> Result<(), ModelError> {
        if self.warp {
            Ok(())
        } else {
            Err(ModelError::InvalidInput(
                "the clip isn't warped: switch Warp on first",
            ))
        }
    }

    /// The clip with the same markers replaced by `markers` and its tempo by
    /// `tempo`, if they make a map (increasing order, a tempo in range).
    fn with_map(&self, markers: Vec<WarpMarker>, tempo: f64) -> Result<Self, ModelError> {
        let next = Self {
            warp_markers: markers,
            tempo: Some(tempo),
            ..self.clone()
        };
        next.warp_map()?;
        Ok(next)
    }

    /// The clip with Warp switched `on` or off (R1, R3, R4). Switching on a
    /// clip with no warp map gives it one marker at its Start marker on
    /// 1.1.1, at `set_tempo`; the Start and End markers and the loop brace
    /// convert through the map, to the nearest tick when Warp goes on and
    /// exactly when it goes off.
    ///
    /// # Errors
    ///
    /// If the clip's map is damaged.
    pub(crate) fn with_warp(&self, on: bool, set_tempo: Bpm) -> Result<Self, ModelError> {
        if on == self.warp {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        if on {
            if next.warp_markers.is_empty() {
                next.warp_markers = vec![WarpMarker {
                    time: self.start,
                    beat: Ticks::ZERO,
                }];
                next.tempo = Some(set_tempo.get());
            }
            let map = next.warp_map()?;
            next = next.converted(|seconds| map.time_to_beat(seconds), true);
        } else {
            let map = self.warp_map()?;
            next = self.converted(|beat| map.beat_to_time(beat).max(0.0), false);
        }
        next.warp = on;
        Ok(next)
    }

    /// The clip with a marker added (R6). Inserting where a marker already
    /// is changes nothing.
    pub(crate) fn with_marker_inserted(&self, at: MarkerAt) -> Result<Self, ModelError> {
        self.require_warp()?;
        let map = self.warp_map()?;
        let (time, beat) = match at {
            MarkerAt::Beat(beat) => (map.beat_to_time(beat.get() as f64), beat),
            MarkerAt::Time(time) => (time, Ticks(map.time_to_beat(time).round() as i64)),
        };
        if !(time.is_finite() && time >= 0.0) {
            return Err(ModelError::InvalidValue {
                what: "warp marker time",
                value: time,
            });
        }
        if self.warp_markers.iter().any(|m| m.beat == beat) {
            return Ok(self.clone());
        }
        let index = self.warp_markers.partition_point(|m| m.time < time);
        let mut markers = self.warp_markers.clone();
        markers.insert(index, WarpMarker { time, beat });
        let tempo = self.tempo.unwrap_or(MIN_BPM);
        self.with_map(markers, tempo)
    }

    /// The clip with the markers at `indices` moved `by` ticks, each keeping
    /// its time in the file (R7). Moving the last marker sets the clip's
    /// tempo to the new last segment's.
    pub(crate) fn with_markers_moved(
        &self,
        indices: &[usize],
        by: Ticks,
    ) -> Result<Self, ModelError> {
        self.require_warp()?;
        self.check_indices(indices)?;
        let mut markers = self.warp_markers.clone();
        for (i, marker) in markers.iter_mut().enumerate() {
            if indices.contains(&i) {
                marker.beat = marker
                    .beat
                    .checked_add(by)
                    .ok_or(ModelError::PositionOverflow)?;
            }
        }
        let mut tempo = self.tempo.unwrap_or(MIN_BPM);
        let map = self.with_map(markers.clone(), tempo)?.warp_map()?;
        let last = markers.len() - 1;
        if last > 0 && indices.contains(&last) {
            tempo = map.segment_bpm(last - 1);
        }
        self.with_map(markers, tempo)
    }

    /// The clip with marker `index` moved `by` seconds in the file, keeping
    /// its beat (R7).
    pub(crate) fn with_marker_shifted(&self, index: usize, by: f64) -> Result<Self, ModelError> {
        self.require_warp()?;
        self.check_indices(&[index])?;
        let mut markers = self.warp_markers.clone();
        markers[index].time += by;
        if !(markers[index].time.is_finite() && markers[index].time >= 0.0) {
            return Err(ModelError::InvalidValue {
                what: "warp marker time",
                value: markers[index].time,
            });
        }
        self.with_map(markers, self.tempo.unwrap_or(MIN_BPM))
    }

    /// The clip without the markers at `indices` (R8): the segments either
    /// side of each join. A warped clip keeps at least one marker.
    pub(crate) fn with_markers_deleted(&self, indices: &[usize]) -> Result<Self, ModelError> {
        self.require_warp()?;
        self.check_indices(indices)?;
        let markers: Vec<_> = self
            .warp_markers
            .iter()
            .enumerate()
            .filter(|(i, _)| !indices.contains(i))
            .map(|(_, m)| *m)
            .collect();
        if markers.is_empty() {
            return Err(ModelError::InvalidInput(
                "a warped clip keeps at least one warp marker",
            ));
        }
        self.with_map(markers, self.tempo.unwrap_or(MIN_BPM))
    }

    /// The clip with the BPM field set to `bpm` (R9). With a marker named it
    /// is the segment that starts at it: the later markers all move by the
    /// same ticks, so that segment alone changes; for the last marker it is
    /// the clip's tempo. With none, every tempo scales by the same factor.
    pub(crate) fn with_segment_bpm(
        &self,
        marker: Option<usize>,
        bpm: f64,
    ) -> Result<Self, ModelError> {
        self.require_warp()?;
        if !(bpm.is_finite() && bpm > 0.0) {
            return Err(ModelError::InvalidValue {
                what: "segment tempo",
                value: bpm,
            });
        }
        let Some(index) = marker else {
            let first = self.warp_map()?.first_bpm();
            return self.with_tempo_scaled(bpm / first);
        };
        self.check_indices(&[index])?;
        let mut markers = self.warp_markers.clone();
        let mut tempo = self.tempo.unwrap_or(MIN_BPM);
        if let Some(next) = markers.get(index + 1).copied() {
            let here = markers[index];
            let wanted = ticks_in(bpm, next.time - here.time).round();
            if wanted < 1.0 || wanted > i64::MAX as f64 / 2.0 {
                return Err(ModelError::InvalidValue {
                    what: "segment tempo",
                    value: bpm,
                });
            }
            let shift = Ticks(wanted as i64 - (next.beat - here.beat).get());
            for later in &mut markers[index + 1..] {
                later.beat = later
                    .beat
                    .checked_add(shift)
                    .ok_or(ModelError::PositionOverflow)?;
            }
        } else {
            tempo = bpm;
        }
        self.with_map(markers, tempo)
    }

    /// The clip with every segment's tempo and its own multiplied by
    /// `factor` (R10): each marker's distance in ticks from the first marker
    /// is multiplied by it, and the Start and End markers and loop brace move
    /// with them about the first marker, so each keeps its place in the
    /// audio.
    pub(crate) fn with_tempo_scaled(&self, factor: f64) -> Result<Self, ModelError> {
        self.require_warp()?;
        if !(factor.is_finite() && factor > 0.0) {
            return Err(ModelError::InvalidValue {
                what: "tempo factor",
                value: factor,
            });
        }
        let first = self.warp_markers[0].beat.get() as f64;
        let scale = |beat: f64| first + ((beat - first) * factor).round();
        let markers = self
            .warp_markers
            .iter()
            .map(|m| WarpMarker {
                time: m.time,
                beat: Ticks(scale(m.beat.get() as f64) as i64),
            })
            .collect();
        let tempo = self.tempo.unwrap_or(MIN_BPM) * factor;
        if !(MIN_BPM..=MAX_BPM).contains(&tempo) {
            return Err(ModelError::InvalidValue {
                what: "clip tempo",
                value: tempo,
            });
        }
        Ok(Self {
            warp_markers: self.with_map(markers, tempo)?.warp_markers,
            tempo: Some(tempo),
            ..self.converted(scale, true)
        })
    }

    /// Checks that `indices` name markers of the clip.
    fn check_indices(&self, indices: &[usize]) -> Result<(), ModelError> {
        if indices.is_empty() || indices.iter().any(|&i| i >= self.warp_markers.len()) {
            return Err(ModelError::InvalidInput(
                "name warp markers by their place in the map, the first being 0",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::Bpm;
    use serde_json::json;

    use super::*;
    use crate::{Clip, ClipData, Command, ContentHash, Editor, TrackKind};

    const Q: i64 = 705_600;

    fn marker(time: f64, beat: i64) -> WarpMarker {
        WarpMarker {
            time,
            beat: Ticks(beat),
        }
    }

    /// A warped clip with these markers (time, beat) and tempo, and the
    /// default Start, End and loop.
    fn warped(markers: &[(f64, i64)], tempo: f64) -> AudioData {
        AudioData {
            warp: true,
            warp_markers: markers.iter().map(|&(t, b)| marker(t, b)).collect(),
            tempo: Some(tempo),
            ..AudioData::new(ContentHash([1; 32]))
        }
    }

    fn beats(audio: &AudioData) -> Vec<i64> {
        audio.warp_markers.iter().map(|m| m.beat.get()).collect()
    }

    fn times(audio: &AudioData) -> Vec<f64> {
        audio.warp_markers.iter().map(|m| m.time).collect()
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn bpm(value: f64) -> Bpm {
        Bpm::new(value).unwrap()
    }

    // Spec: warping R1
    #[test]
    fn the_warp_switch_is_one_undo_step_and_saved() {
        let mut editor = Editor::new();
        let track = editor.new_id();
        let clip = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let data = ClipData::Audio(AudioData::new(ContentHash([1; 32])));
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, data),
            })
            .unwrap();
        let audio = |editor: &Editor| match &editor.document().clip(clip).unwrap().data {
            ClipData::Audio(audio) => audio.clone(),
            ClipData::Midi(_) => unreachable!("an audio clip"),
        };
        let unwarped = audio(&editor);
        // An unwarped clip saves none of the warp fields, so older sets load
        // as they are.
        let saved = serde_json::to_value(&unwarped).unwrap();
        for field in ["warp", "warp_markers", "tempo", "signature"] {
            assert!(saved.get(field).is_none(), "{field}");
        }
        editor
            .apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        let on = audio(&editor);
        assert!(on.warp);
        let saved = serde_json::to_value(&on).unwrap();
        assert_eq!(saved["warp"], json!(true));
        assert_eq!(saved["warp_markers"][0], json!({ "time": 0.0, "beat": 0 }));
        assert_eq!(saved["tempo"], json!(120.0));
        assert_eq!(serde_json::from_value::<AudioData>(saved).unwrap(), on);
        editor.undo().unwrap();
        assert_eq!(audio(&editor), unwarped, "one undo step");
        editor.redo().unwrap();
        assert_eq!(audio(&editor), on);
    }

    // Spec: warping R11. A warped clip's markers, Start and End and brace
    // are beats, so a new tempo changes none of them.
    #[test]
    fn a_warped_clips_markers_stay_on_their_beats_through_a_tempo_change() {
        let mut editor = Editor::new();
        let (track, clip) = (editor.new_id(), editor.new_id());
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let data = ClipData::Audio(AudioData::new(ContentHash([1; 32])));
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, Ticks::WHOLE, Ticks::WHOLE * 2, data),
            })
            .unwrap();
        editor
            .apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        editor
            .apply(Command::InsertWarpMarker {
                clips: vec![clip],
                beat: Some(Ticks::WHOLE),
                time: None,
            })
            .unwrap();
        let before = editor.document().clip(clip).unwrap().clone();
        let tempo = bl_core::time::TempoMap::from_breakpoints([
            (Ticks::ZERO, bpm(90.0)),
            (Ticks::WHOLE * 2, bpm(140.0)),
        ])
        .unwrap();
        editor.apply(Command::SetTempo { tempo }).unwrap();
        assert_eq!(*editor.document().clip(clip).unwrap(), before);
    }

    // Spec: warping R3
    #[test]
    fn warping_a_clip_keeps_its_speed_and_unwarping_keeps_its_map() {
        let plain = AudioData {
            start: 0.5,
            end: Some(2.0),
            loop_start: 0.25,
            loop_end: Some(1.5),
            ..AudioData::new(ContentHash([1; 32]))
        };
        let on = plain.with_warp(true, bpm(120.0)).unwrap();
        // One marker at the Start marker on 1.1.1, at the set's tempo.
        assert_eq!(
            (times(&on), beats(&on), on.tempo),
            (vec![0.5], vec![0], Some(120.0))
        );
        let map = on.warp_map().unwrap();
        assert!(near(map.first_bpm(), 120.0) && near(map.bpm(), 120.0));
        // At 120 BPM a second is two quarter notes: the clip's speed is as it was.
        assert_eq!(
            (on.start, on.end, on.loop_start, on.loop_end),
            (
                0.0,
                Some(3.0 * Q as f64),
                -(Q as f64) / 2.0,
                Some(2.0 * Q as f64)
            )
        );
        let off = on.with_warp(false, bpm(90.0)).unwrap();
        assert_eq!(
            AudioData {
                warp: true,
                ..off.clone()
            }
            .warp_markers,
            on.warp_markers,
            "switching off keeps the map"
        );
        assert!(!off.warp);
        for (a, b) in [
            (off.start, plain.start),
            (off.end.unwrap(), plain.end.unwrap()),
            (off.loop_start.max(0.0), plain.loop_start),
            (off.loop_end.unwrap(), plain.loop_end.unwrap()),
        ] {
            assert!(near(a, b), "{a} is not {b}");
        }
        // On again: the map comes back whatever the set's tempo is now.
        let again = off.with_warp(true, bpm(90.0)).unwrap();
        assert_eq!((again.tempo, again.warp_markers.len()), (Some(120.0), 1));
    }

    // Spec: warping R4
    #[test]
    fn switching_warp_converts_the_markers_and_brace_through_the_map() {
        // 120 BPM to 1.5 s, then a quarter note a second (60 BPM), then the
        // clip's 90.
        let map = [(0.5, 0), (1.5, 2 * Q), (2.5, 3 * Q)];
        let plain = AudioData {
            start: 1.0,
            end: Some(2.0),
            looping: true,
            loop_start: 0.75,
            loop_end: Some(3.25),
            ..warped(&map, 90.0)
        };
        let plain = AudioData {
            warp: false,
            ..plain
        };
        let on = plain.with_warp(true, bpm(100.0)).unwrap();
        assert_eq!(on.start, Q as f64, "half a second into a 120 BPM segment");
        assert_eq!(
            on.end,
            Some(2.5 * Q as f64),
            "half a quarter into the 60 BPM one"
        );
        assert_eq!(on.loop_start, 0.5 * Q as f64);
        // 0.75 s past the last marker, at 90 BPM: a quarter and a half.
        assert_eq!(on.loop_end, Some((3.0 + 1.125) * Q as f64));
        let off = on.with_warp(false, bpm(100.0)).unwrap();
        assert!(near(off.start, 1.0) && near(off.end.unwrap(), 2.0));
        assert!(near(off.loop_start, 0.75) && near(off.loop_end.unwrap(), 3.25));
        // An open end stays open.
        let open = AudioData {
            end: None,
            loop_end: None,
            ..plain
        };
        let on = open.with_warp(true, bpm(100.0)).unwrap();
        assert_eq!((on.end, on.loop_end), (None, None));
    }

    // Spec: warping R4
    #[test]
    fn set_region_takes_the_form_the_clip_shows() {
        let mut editor = Editor::new();
        let (track, clip) = (editor.new_id(), editor.new_id());
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let data = ClipData::Audio(AudioData::new(ContentHash([1; 32])));
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, data),
            })
            .unwrap();
        let region = |start: f64, ticks: bool| Command::SetClipRegion {
            clip,
            start,
            end: None,
            ticks,
        };
        assert!(
            editor.apply(region(1.0, true)).is_err(),
            "ticks of an unwarped clip"
        );
        editor.apply(region(1.0, false)).unwrap();
        editor
            .apply(Command::SetClipWarp { clip, warp: true })
            .unwrap();
        assert!(
            editor.apply(region(1.0, false)).is_err(),
            "seconds of a warped clip"
        );
        assert!(editor.apply(region(0.5, true)).is_err(), "half a tick");
        editor.apply(region(Q as f64, true)).unwrap();
        let loop_brace = |start: f64, ticks: bool| Command::SetClipLoop {
            clip,
            start,
            end: Some(start + 2.0 * Q as f64),
            ticks,
        };
        assert!(editor.apply(loop_brace(0.0, false)).is_err());
        editor.apply(loop_brace(0.0, true)).unwrap();
    }

    // Spec: warping R6
    #[test]
    fn inserting_a_marker_moves_nothing() {
        let before = warped(&[(0.5, 0), (1.5, 2 * Q), (2.5, 3 * Q)], 90.0);
        let map = before.warp_map().unwrap();
        for after in [
            before
                .with_marker_inserted(MarkerAt::Beat(Ticks(Q)))
                .unwrap(),
            before.with_marker_inserted(MarkerAt::Time(2.0)).unwrap(),
            // Past the ends, too.
            before
                .with_marker_inserted(MarkerAt::Beat(Ticks(-Q)))
                .unwrap(),
            before
                .with_marker_inserted(MarkerAt::Beat(Ticks(9 * Q)))
                .unwrap(),
        ] {
            assert_eq!(after.warp_markers.len(), 4);
            assert!(after.warp_markers.windows(2).all(|p| p[0].beat < p[1].beat));
            let now = after.warp_map().unwrap();
            for beat in [-3 * Q, -Q, 0, Q, 2 * Q, 5 * Q / 2, 3 * Q, 5 * Q, 20 * Q] {
                let beat = beat as f64;
                assert!(
                    near(now.beat_to_time(beat), map.beat_to_time(beat)),
                    "{beat}"
                );
            }
            assert_eq!(now.bpm(), map.bpm());
        }
        // The marker is named by its place in the map.
        let after = before
            .with_marker_inserted(MarkerAt::Beat(Ticks(Q)))
            .unwrap();
        assert_eq!(beats(&after), [0, Q, 2 * Q, 3 * Q]);
        assert!(near(after.warp_markers[1].time, 1.0));
        // Where a marker is already: nothing changes.
        assert_eq!(
            before
                .with_marker_inserted(MarkerAt::Beat(Ticks(2 * Q)))
                .unwrap(),
            before
        );
        assert_eq!(
            before.with_marker_inserted(MarkerAt::Time(1.5)).unwrap(),
            before
        );
        // Not in the file, and not on an unwarped clip.
        assert!(
            before
                .with_marker_inserted(MarkerAt::Beat(Ticks(-5 * Q)))
                .is_err()
        );
        assert!(before.with_marker_inserted(MarkerAt::Time(-1.0)).is_err());
        let off = AudioData {
            warp: false,
            ..before
        };
        assert!(off.with_marker_inserted(MarkerAt::Beat(Ticks(Q))).is_err());
    }

    // Spec: warping R7
    #[test]
    fn moving_markers_keeps_their_file_times_and_order() {
        let before = warped(&[(0.5, 0), (1.5, 2 * Q), (2.5, 3 * Q)], 90.0);
        let moved = before.with_markers_moved(&[1], Ticks(Q / 2)).unwrap();
        assert_eq!(beats(&moved), [0, 2 * Q + Q / 2, 3 * Q]);
        assert_eq!(
            times(&moved),
            times(&before),
            "each keeps its time in the file"
        );
        assert_eq!(moved.tempo, before.tempo, "the last marker didn't move");
        // Together, the segment between them keeps its length.
        let both = before.with_markers_moved(&[0, 1], Ticks(-Q)).unwrap();
        assert_eq!(beats(&both), [-Q, Q, 3 * Q]);
        // Passing or reaching a neighbour that isn't moving is refused.
        for (indices, by) in [
            (&[1][..], 2 * Q),
            (&[1], Q),
            (&[1], -2 * Q),
            (&[0], 2 * Q),
            (&[2], -Q),
        ] {
            assert!(
                before.with_markers_moved(indices, Ticks(by)).is_err(),
                "{indices:?} {by}"
            );
        }
        assert!(
            before.with_markers_moved(&[3], Ticks(1)).is_err(),
            "no such marker"
        );
        assert!(before.with_markers_moved(&[], Ticks(1)).is_err());
        // Shifting moves the time in the file, keeping the beat.
        let shifted = before.with_marker_shifted(1, 0.25).unwrap();
        assert_eq!(
            (beats(&shifted), shifted.warp_markers[1].time),
            (beats(&before), 1.75)
        );
        for by in [1.0, -1.0, -2.0] {
            assert!(before.with_marker_shifted(1, by).is_err(), "{by}");
        }
        assert!(
            before.with_marker_shifted(0, -0.75).is_err(),
            "out of the file"
        );
    }

    // Spec: warping R7
    #[test]
    fn moving_the_last_marker_sets_the_clips_tempo() {
        let before = warped(&[(0.0, 0), (1.0, 2 * Q)], 60.0);
        // A quarter note a second more: 4 quarter notes in the last second.
        let moved = before.with_markers_moved(&[1], Ticks(2 * Q)).unwrap();
        assert!(near(moved.tempo.unwrap(), 240.0), "{:?}", moved.tempo);
        // The audio after the marker goes on at the segment before it.
        let map = moved.warp_map().unwrap();
        assert!(near(
            map.beat_to_time(8.0 * Q as f64),
            1.0 + 4.0 * 60.0 / 240.0
        ));
        // Moving another marker leaves the tempo alone, and so does a lone marker.
        let first = before.with_markers_moved(&[0], Ticks(-Q)).unwrap();
        assert_eq!(first.tempo, Some(60.0));
        let lone = warped(&[(0.0, 0)], 60.0)
            .with_markers_moved(&[0], Ticks(Q))
            .unwrap();
        assert_eq!(lone.tempo, Some(60.0));
        // A tempo the tempo map would refuse is refused.
        assert!(before.with_markers_moved(&[1], Ticks(10_000 * Q)).is_err());
    }

    // Spec: warping R8
    #[test]
    fn deleting_joins_the_segments_and_keeps_one_marker() {
        let before = warped(&[(0.0, 0), (1.0, 2 * Q), (3.0, 3 * Q)], 90.0);
        let joined = before.with_markers_deleted(&[1]).unwrap();
        assert_eq!(beats(&joined), [0, 3 * Q]);
        // One straight segment between the neighbours: 3 quarters in 3 s.
        assert!(near(joined.warp_map().unwrap().segment_bpm(0), 60.0));
        // A deleted last marker leaves the tail at the clip's tempo.
        let tail = before.with_markers_deleted(&[2]).unwrap();
        assert_eq!((beats(&tail), tail.tempo), (vec![0, 2 * Q], Some(90.0)));
        let map = tail.warp_map().unwrap();
        assert!(near(map.beat_to_time(3.0 * Q as f64), 1.0 + 60.0 / 90.0));
        // The first marker goes: the new first segment runs on before it.
        let head = before.with_markers_deleted(&[0, 1]).unwrap();
        assert_eq!(beats(&head), [3 * Q]);
        // The only marker, or every marker, stays.
        assert!(head.with_markers_deleted(&[0]).is_err());
        assert!(before.with_markers_deleted(&[0, 1, 2]).is_err());
        assert!(before.with_markers_deleted(&[3]).is_err());
    }

    // Spec: warping R9
    #[test]
    fn a_segments_bpm_moves_only_the_later_markers() {
        let before = warped(&[(0.0, 0), (1.0, 2 * Q), (2.0, 5 * Q)], 90.0);
        let bpms = |audio: &AudioData| {
            let map = audio.warp_map().unwrap();
            [map.segment_bpm(0), map.segment_bpm(1), map.segment_bpm(2)]
        };
        assert_eq!(bpms(&before), [120.0, 180.0, 90.0]);
        // The first segment at 60: 1 quarter in a second; the later markers
        // move by the same ticks and keep their segments' tempos.
        let slower = before.with_segment_bpm(Some(0), 60.0).unwrap();
        assert_eq!(beats(&slower), [0, Q, 4 * Q]);
        assert_eq!(bpms(&slower), [60.0, 180.0, 90.0]);
        // The last marker's segment is the clip's tempo.
        let last = before.with_segment_bpm(Some(2), 140.0).unwrap();
        assert_eq!((beats(&last), last.tempo), (beats(&before), Some(140.0)));
        assert!(before.with_segment_bpm(Some(2), 1_500.0).is_err());
        // With none named, the field shows the first segment's and sets every one.
        let scaled = before.with_segment_bpm(None, 240.0).unwrap();
        assert_eq!(bpms(&scaled), [240.0, 360.0, 180.0]);
        assert_eq!(scaled.tempo, Some(180.0));
        // A lone marker's field is the clip's tempo.
        let lone = warped(&[(0.0, 0)], 90.0)
            .with_segment_bpm(None, 120.0)
            .unwrap();
        assert_eq!(lone.tempo, Some(120.0));
        assert!(before.with_segment_bpm(Some(1), 0.0).is_err());
        assert!(before.with_segment_bpm(Some(3), 100.0).is_err());
    }

    // Spec: warping R10
    #[test]
    fn doubling_and_halving_scale_the_beats_about_the_first_marker() {
        let before = AudioData {
            start: 2.0 * Q as f64,
            end: Some(4.0 * Q as f64),
            looping: true,
            loop_start: 0.0,
            loop_end: Some(3.0 * Q as f64),
            ..warped(&[(0.5, Q), (1.5, 3 * Q), (2.5, 6 * Q)], 100.0)
        };
        let map = before.warp_map().unwrap();
        let double = before.with_tempo_scaled(2.0).unwrap();
        // Distances from the first marker double; every tempo doubles.
        assert_eq!(beats(&double), [Q, 5 * Q, 11 * Q]);
        assert_eq!(double.tempo, Some(200.0));
        let now = double.warp_map().unwrap();
        for i in 0..3 {
            assert!(near(now.segment_bpm(i), 2.0 * map.segment_bpm(i)), "{i}");
        }
        // The Start and End markers and the brace keep their place in the audio.
        let place = |audio: &AudioData, beat: f64| audio.warp_map().unwrap().beat_to_time(beat);
        assert!(near(
            place(&double, double.start),
            place(&before, before.start)
        ));
        assert!(near(
            place(&double, double.end.unwrap()),
            place(&before, before.end.unwrap())
        ));
        assert!(near(
            place(&double, double.loop_start),
            place(&before, before.loop_start)
        ));
        assert!(near(
            place(&double, double.loop_end.unwrap()),
            place(&before, before.loop_end.unwrap())
        ));
        // Halving comes back.
        assert_eq!(double.with_tempo_scaled(0.5).unwrap(), before);
        // A tempo out of range is refused.
        assert!(before.with_tempo_scaled(20.0).is_err());
        assert!(before.with_tempo_scaled(0.0).is_err());
    }

    #[test]
    fn warp_edits_refuse_an_unwarped_clip() {
        let off = AudioData {
            warp: false,
            ..warped(&[(0.0, 0), (1.0, Q)], 60.0)
        };
        assert!(off.with_markers_moved(&[0], Ticks(1)).is_err());
        assert!(off.with_markers_deleted(&[0]).is_err());
        assert!(off.with_segment_bpm(None, 90.0).is_err());
        assert!(off.with_tempo_scaled(2.0).is_err());
        // Its Start and End stay in seconds for the engine.
        assert_eq!(off.in_seconds().into_owned(), off);
    }

    #[test]
    fn the_engine_reads_a_warped_clip_in_seconds() {
        let on = AudioData {
            start: Q as f64,
            end: Some(2.0 * Q as f64),
            ..warped(&[(0.5, 0), (1.5, 2 * Q)], 90.0)
        };
        let seconds = on.in_seconds();
        assert!(near(seconds.start, 1.0) && near(seconds.end.unwrap(), 1.5));
        assert_eq!(on.in_seconds().warp_markers, on.warp_markers);
    }
}

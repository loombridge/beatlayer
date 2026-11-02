//! Opening and closing time on every track at once: what `InsertTime` and
//! `DeleteTime` do, and the one place a new kind of time-placed object
//! joins (time editing R14 to R17; the architecture's "The document").

use bl_core::time::{Bpm, TempoMap, Ticks, TimeSignatureChange};

use super::{tempo_breakpoints, tempo_from_breakpoints, time_signature_map};
use crate::clip::Clip;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ClipId, TakeLaneId, TrackId};

/// Where an Arrangement clip sits: its track, and its take lane if it's on
/// one.
type Place = (TrackId, Option<TakeLaneId>);

impl Document {
    /// Every Arrangement clip (the main lanes and the take lanes of every
    /// track), in a fixed order, with where it sits.
    fn arrangement_clips(&self) -> Vec<(Place, &Clip)> {
        let mut clips = Vec::new();
        for track in self.tracks() {
            clips.extend(track.clips().map(|clip| ((track.id(), None), clip)));
            for lane in track.take_lanes() {
                clips.extend(
                    lane.clips
                        .iter()
                        .map(|clip| ((track.id(), Some(lane.id)), clip)),
                );
            }
        }
        clips
    }

    /// Every Arrangement clip's place, ID, start and end.
    fn arrangement_spans(&self) -> Vec<(Place, ClipId, Ticks, Ticks)> {
        self.arrangement_clips()
            .into_iter()
            .map(|(place, clip)| (place, clip.id, clip.start, clip.end()))
            .collect()
    }

    /// The clips that opening time at `at` splits in two: those that start
    /// before it and end after it. `InsertTime` takes one new ID for each,
    /// in this order.
    #[must_use]
    pub fn clips_split_by_insert(&self, at: Ticks) -> Vec<ClipId> {
        self.arrangement_clips()
            .into_iter()
            .filter(|(_, clip)| clip.start < at && clip.end() > at)
            .map(|(_, clip)| clip.id)
            .collect()
    }

    /// The clips that closing `start..end` splits in two: those that start
    /// before it and end after it. `DeleteTime` takes one new ID for each,
    /// in this order.
    #[must_use]
    pub fn clips_split_by_delete(&self, start: Ticks, end: Ticks) -> Vec<ClipId> {
        self.arrangement_clips()
            .into_iter()
            .filter(|(_, clip)| clip.start < start && clip.end() > end)
            .map(|(_, clip)| clip.id)
            .collect()
    }

    /// Opens `length` of empty time at `at` on every track: a clip across
    /// `at` is split there, the part after it taking the next of `ids`, and
    /// everything placed at or after `at` moves later by `length`; each
    /// envelope, the tempo and the signature hold across the new time what
    /// they had at `at` (time editing R17).
    pub(super) fn insert_time(
        &mut self,
        at: Ticks,
        length: Ticks,
        ids: &[ClipId],
    ) -> Result<(), ModelError> {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        if length <= Ticks::ZERO {
            return Err(ModelError::InvalidInput(
                "the time to open must be longer than 0",
            ));
        }
        let opened_end = at.checked_add(length).ok_or(ModelError::PositionOverflow)?;
        if ids.len() != self.clips_split_by_insert(at).len() {
            return Err(ModelError::InvalidInput(
                "`clips` needs one new ID for each clip the time opens inside, in track order",
            ));
        }
        let tempo = self.settings.tempo.clone();
        let (mut later, mut split) = (Vec::new(), Vec::new());
        for (place, id, clip_start, clip_end) in self.arrangement_spans() {
            if clip_start >= at {
                later.push(id);
            } else if clip_end > at {
                split.push((place, self.clip_ref(id)?.clone()));
            }
        }
        self.move_clips(&later, length)?;
        for ((place, clip), id) in split.into_iter().zip(ids) {
            let right = Clip {
                id: *id,
                start: opened_end,
                ..clip.part(at, clip.end(), &tempo)
            };
            self.clip_mut(clip.id)?.length = at - clip.start;
            self.add_arrangement_clip(place, &right)?;
        }
        self.each_envelope(|envelope| envelope.insert_time(at, length))?;
        self.insert_section_time(at, length)?;

        let mut points = tempo_breakpoints(&tempo);
        insert_tempo_time(&mut points, at, length)?;
        self.set_tempo_points(points);

        let changes = self
            .settings
            .time_signatures
            .changes()
            .map(|change| {
                let moved = if change.at >= at && change.at > Ticks::ZERO {
                    change
                        .at
                        .checked_add(length)
                        .ok_or(ModelError::PositionOverflow)?
                } else {
                    change.at
                };
                Ok(TimeSignatureChange {
                    at: moved,
                    ..change
                })
            })
            .collect::<Result<_, ModelError>>()?;
        self.settings.time_signatures = time_signature_map(changes);
        for locator in &mut self.settings.locators {
            if locator.position >= at {
                locator.position = locator
                    .position
                    .checked_add(length)
                    .ok_or(ModelError::PositionOverflow)?;
            }
        }
        Ok(())
    }

    /// Closes `start..end` on every track: the clips and parts of clips
    /// inside it go, a clip across an edge is cut or split there (the part
    /// after a split taking the next of `ids`), and everything placed at or
    /// after `end` moves earlier by the span's length (time editing R15).
    pub(super) fn delete_time(
        &mut self,
        start: Ticks,
        end: Ticks,
        ids: &[ClipId],
    ) -> Result<(), ModelError> {
        if start < Ticks::ZERO {
            return Err(ModelError::NegativePosition(start));
        }
        if end <= start {
            return Err(ModelError::InvalidInput(
                "the span's end must be after its start",
            ));
        }
        if ids.len() != self.clips_split_by_delete(start, end).len() {
            return Err(ModelError::InvalidInput(
                "`clips` needs one new ID for each clip the span lies inside, in track order",
            ));
        }
        let length = end - start;
        let tempo = self.settings.tempo.clone();
        let (mut earlier, mut gone, mut split, mut cut_front) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (place, id, clip_start, clip_end) in self.arrangement_spans() {
            if clip_end <= start {
                continue;
            }
            if clip_start >= end {
                earlier.push(id);
            } else if clip_start >= start && clip_end <= end {
                gone.push(id);
            } else if clip_start < start && clip_end > end {
                split.push((place, self.clip_ref(id)?.clone()));
            } else if clip_start < start {
                self.clip_mut(id)?.length = start - clip_start;
            } else {
                cut_front.push(self.clip_ref(id)?.clone());
            }
        }
        for clip in gone {
            self.remove_clip(clip)?;
        }
        for clip in cut_front {
            *self.clip_mut(clip.id)? = Clip {
                start,
                ..clip.part(end, clip.end(), &tempo)
            };
        }
        for ((place, clip), id) in split.into_iter().zip(ids) {
            let right = Clip {
                id: *id,
                start,
                ..clip.part(end, clip.end(), &tempo)
            };
            self.clip_mut(clip.id)?.length = start - clip.start;
            self.add_arrangement_clip(place, &right)?;
        }
        self.move_clips(&earlier, -length)?;
        self.each_envelope(|envelope| {
            envelope.delete_time(start, end);
            Ok(())
        })?;

        let mut points = tempo_breakpoints(&tempo);
        delete_tempo_time(&mut points, start, end);
        self.set_tempo_points(points);

        self.delete_signature_time(start, end);
        self.delete_section_time(start, end);
        let inside: Vec<_> = self
            .settings
            .locators
            .iter()
            .filter(|locator| locator.position >= start && locator.position < end)
            .map(|locator| locator.id)
            .collect();
        for locator in inside {
            self.delete_locator(locator)?;
        }
        for locator in &mut self.settings.locators {
            if locator.position >= end {
                locator.position -= length;
            }
        }
        Ok(())
    }

    /// Time-signature markers for a closed span: those inside go, those
    /// from `end` on move earlier, and the signature in force at `end`
    /// stays in force from `start`, with a marker there where it differs
    /// from the one before (time editing R15).
    fn delete_signature_time(&mut self, start: Ticks, end: Ticks) {
        let map = &self.settings.time_signatures;
        let length = end - start;
        let after = map.signature_at(end);
        let mut changes: Vec<TimeSignatureChange> = map
            .changes()
            .filter(|change| change.at < start || change.at >= end)
            .map(|change| TimeSignatureChange {
                at: if change.at >= end {
                    change.at - length
                } else {
                    change.at
                },
                ..change
            })
            .collect();
        let before = changes.iter().rev().find(|change| change.at < start);
        if !changes.iter().any(|change| change.at == start)
            && before.is_none_or(|before| before.signature != after)
        {
            changes.push(TimeSignatureChange {
                at: start,
                signature: after,
            });
            changes.sort_by_key(|change| change.at);
        }
        self.settings.time_signatures = time_signature_map(changes);
    }

    /// Adds `clip` to the main lane or take lane `place` names.
    fn add_arrangement_clip(&mut self, place: Place, clip: &Clip) -> Result<(), ModelError> {
        match place {
            (track, None) => self.create_clip(track, clip),
            (_, Some(lane)) => self.create_take_clip(lane, clip),
        }
    }

    /// Runs `edit` on every envelope of every track, the Main track's and the
    /// returns' included.
    fn each_envelope(
        &mut self,
        edit: impl Fn(&mut crate::automation::Envelope) -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        let holders: Vec<TrackId> = self
            .tracks()
            .filter(|track| !track.automation.is_empty())
            .map(|track| track.id())
            .collect();
        for id in holders {
            let track = self.track_mut(id)?;
            let targets: Vec<_> = track.automation.keys().copied().collect();
            for target in targets {
                if let Some(automation) = track.automation.get_mut(&target) {
                    edit(&mut automation.envelope)?;
                }
            }
        }
        Ok(())
    }

    /// The tempo map `points` make, set if it differs from the set's.
    fn set_tempo_points(&mut self, points: Vec<(Ticks, Bpm)>) {
        let next = tempo_from_breakpoints(points);
        if next != self.settings.tempo {
            self.settings.tempo = next;
        }
    }
}

/// The tempo arriving at `at`: the straight line between the breakpoints
/// round it, where a jump's first breakpoint is the one arriving.
fn tempo_arriving(points: &[(Ticks, Bpm)], at: Ticks) -> Bpm {
    let next = points.partition_point(|&(position, _)| position < at);
    match (next.checked_sub(1).map(|i| points[i]), points.get(next)) {
        (Some(from), Some(&to)) => along(from, to, at),
        (None, Some(&(_, bpm))) => bpm,
        (_, None) => points.last().map_or(Bpm::DEFAULT, |&(_, bpm)| bpm),
    }
}

/// The tempo leaving `at`: the last breakpoint at `at`, or the line.
fn tempo_leaving(points: &[(Ticks, Bpm)], at: Ticks) -> Bpm {
    let next = points.partition_point(|&(position, _)| position <= at);
    match (next.checked_sub(1).map(|i| points[i]), points.get(next)) {
        (Some(from), Some(&to)) => along(from, to, at),
        (None, Some(&(_, bpm))) => bpm,
        (_, None) => points.last().map_or(Bpm::DEFAULT, |&(_, bpm)| bpm),
    }
}

/// The tempo `at` of the way from one breakpoint to the next.
fn along(from: (Ticks, Bpm), to: (Ticks, Bpm), at: Ticks) -> Bpm {
    let span = (to.0 - from.0).get() as f64;
    if span <= 0.0 {
        return to.1;
    }
    let part = (at - from.0).get() as f64 / span;
    Bpm::new(from.1.get() + (to.1.get() - from.1.get()) * part).unwrap_or(from.1)
}

/// Opens `length` at `at` in a tempo's breakpoints: those from `at` on move
/// later, and the tempo arriving at `at` holds across the new time.
fn insert_tempo_time(
    points: &mut Vec<(Ticks, Bpm)>,
    at: Ticks,
    length: Ticks,
) -> Result<(), ModelError> {
    let before = points.partition_point(|&(position, _)| position < at);
    if before == points.len() {
        return Ok(());
    }
    let held = (before > 0).then(|| tempo_arriving(points, at));
    for point in &mut points[before..] {
        point.0 = point
            .0
            .checked_add(length)
            .ok_or(ModelError::PositionOverflow)?;
    }
    if let Some(bpm) = held {
        let mut hold = vec![(at, bpm)];
        if points[before].0 != at + length {
            hold.push((at + length, bpm));
        }
        points.splice(before..before, hold);
    }
    Ok(())
}

/// Closes `start..end` in a tempo's breakpoints: those inside go, those
/// from `end` on move earlier, and the tempo jumps at `start` from the one
/// arriving there to the one leaving `end`, with two breakpoints at one
/// position where they differ (time editing R15).
fn delete_tempo_time(points: &mut Vec<(Ticks, Bpm)>, start: Ticks, end: Ticks) {
    if points.last().is_none_or(|&(position, _)| position < start) {
        return;
    }
    let (arriving, leaving) = (tempo_arriving(points, start), tempo_leaving(points, end));
    let length = end - start;
    let mut closed: Vec<(Ticks, Bpm)> = points
        .iter()
        .copied()
        .filter(|&(position, _)| position < start)
        .collect();
    closed.push((start, arriving));
    if leaving != arriving {
        closed.push((start, leaving));
    }
    closed.extend(
        points
            .iter()
            .filter(|&&(position, _)| position > end)
            .map(|&(position, bpm)| (position - length, bpm)),
    );
    *points = closed;
}

/// The shape of a tempo over `start..end`, for Copy Time: breakpoints with
/// positions from `start`, the tempo leaving `start` first, the breakpoints
/// inside, and the tempo arriving at `end` last.
#[must_use]
pub fn tempo_shape(tempo: &TempoMap, start: Ticks, end: Ticks) -> Vec<(Ticks, Bpm)> {
    let points = tempo_breakpoints(tempo);
    let mut shape = vec![(Ticks::ZERO, tempo_leaving(&points, start))];
    shape.extend(
        points
            .iter()
            .filter(|&&(position, _)| position > start && position < end)
            .map(|&(position, bpm)| (position - start, bpm)),
    );
    shape.push((end - start, tempo_arriving(&points, end)));
    shape
}

/// `tempo` with `shape` (from [`tempo_shape`]) written over `start..end`:
/// the breakpoints inside go and the shape's take their place. A breakpoint
/// already at `end` takes the shape's last value.
#[must_use]
pub fn write_tempo_shape(
    tempo: &TempoMap,
    start: Ticks,
    end: Ticks,
    shape: &[(Ticks, Bpm)],
) -> TempoMap {
    let points = tempo_breakpoints(tempo);
    let mut written: Vec<(Ticks, Bpm)> = points
        .iter()
        .copied()
        .filter(|&(position, _)| position < start)
        .collect();
    let mut tail: Vec<(Ticks, Bpm)> = points
        .iter()
        .copied()
        .filter(|&(position, _)| position >= end)
        .collect();
    let shape = shape.iter().map(|&(position, bpm)| (start + position, bpm));
    for (position, bpm) in shape {
        match tail.first_mut() {
            Some(first) if position == end && first.0 == end => first.1 = bpm,
            _ => written.push((position, bpm)),
        }
    }
    written.extend(tail);
    tempo_from_breakpoints(written)
}

#[cfg(test)]
mod tests {
    use bl_core::id::IdAllocator;
    use bl_core::time::TimeSignature;

    use super::*;
    use crate::automation::{AutomationTarget, Breakpoint, Envelope, MixerParam};
    use crate::clip::{AudioData, ClipData, MidiData};
    use crate::command::Command;
    use crate::ids::{ContentHash, LocatorId, SceneId};
    use crate::locator::{Locator, SongStart};
    use crate::scene::DEFAULT_SCENE_COUNT;
    use crate::track::TrackKind;

    const BAR: i64 = Ticks::WHOLE.0;

    fn bar(n: i64) -> Ticks {
        Ticks(BAR * n)
    }

    fn envelope(points: &[(i64, f64)]) -> Envelope {
        Envelope::from_points(
            points
                .iter()
                .map(|&(at, value)| Breakpoint {
                    at: bar(at),
                    value,
                    bend: 0.0,
                })
                .collect(),
        )
        .unwrap()
    }

    fn shape(envelope: &Envelope) -> Vec<(Ticks, f64)> {
        envelope.points().map(|p| (p.at, p.value)).collect()
    }

    // Spec: time-editing R15
    #[test]
    fn deleting_time_from_an_envelope_keeps_the_shape_either_side_and_steps_between() {
        // A ramp from 0 to 1 over four bars loses its middle two bars.
        let mut ramp = envelope(&[(0, 0.0), (4, 1.0)]);
        ramp.delete_time(bar(1), bar(3));
        assert_eq!(
            shape(&ramp),
            [
                (bar(0), 0.0),
                (bar(1), 0.25),
                (bar(1) + Ticks(1), 0.75),
                (bar(2), 1.0)
            ]
        );
        // Where the two sides meet at one value, there is no step.
        let mut flat = envelope(&[(0, 0.5), (2, 0.5), (4, 0.5)]);
        flat.delete_time(bar(1), bar(3));
        assert_eq!(shape(&flat), [(bar(0), 0.5), (bar(1), 0.5), (bar(2), 0.5)]);
        // A span after the last breakpoint changes nothing.
        let mut later = envelope(&[(0, 0.0), (1, 1.0)]);
        later.delete_time(bar(2), bar(3));
        assert_eq!(shape(&later), [(bar(0), 0.0), (bar(1), 1.0)]);
        // A breakpoint at the span's end moves up to a tick after the start.
        let mut at_end = envelope(&[(0, 0.0), (2, 1.0), (4, 0.0)]);
        at_end.delete_time(bar(1), bar(2));
        assert_eq!(
            shape(&at_end),
            [
                (bar(0), 0.0),
                (bar(1), 0.5),
                (bar(1) + Ticks(1), 1.0),
                (bar(3), 0.0)
            ]
        );
    }

    // Spec: time-editing R17
    #[test]
    fn inserting_time_in_an_envelope_holds_the_value_it_had_there() {
        let mut ramp = envelope(&[(0, 0.0), (4, 1.0)]);
        ramp.insert_time(bar(2), bar(1)).unwrap();
        assert_eq!(
            shape(&ramp),
            [(bar(0), 0.0), (bar(2), 0.5), (bar(3), 0.5), (bar(5), 1.0)]
        );
        // At a breakpoint, it moves later and the value before it holds.
        let mut stepped = envelope(&[(0, 0.0), (2, 1.0)]);
        stepped.insert_time(bar(2), bar(1)).unwrap();
        assert_eq!(
            shape(&stepped),
            [(bar(0), 0.0), (bar(2), 1.0), (bar(3), 1.0)]
        );
        // Before the first breakpoint and after the last, the envelope is
        // flat already.
        let mut ends = envelope(&[(1, 0.2), (2, 0.8)]);
        ends.insert_time(bar(0), bar(1)).unwrap();
        ends.insert_time(bar(9), bar(1)).unwrap();
        assert_eq!(shape(&ends), [(bar(2), 0.2), (bar(3), 0.8)]);
    }

    fn tempo(points: &[(i64, f64)]) -> Vec<(Ticks, Bpm)> {
        points
            .iter()
            .map(|&(at, bpm)| (bar(at), Bpm::new(bpm).unwrap()))
            .collect()
    }

    // Spec: time-editing R15
    #[test]
    fn deleting_time_from_the_tempo_jumps_at_the_start_with_two_breakpoints() {
        let mut points = tempo(&[(0, 100.0), (4, 140.0)]);
        delete_tempo_time(&mut points, bar(1), bar(3));
        assert_eq!(
            points,
            tempo(&[(0, 100.0), (1, 110.0), (1, 130.0), (2, 140.0)])
        );
        // No jump where the tempo is the same either side.
        let mut steady = tempo(&[(0, 120.0), (4, 120.0)]);
        delete_tempo_time(&mut steady, bar(1), bar(3));
        assert_eq!(steady, tempo(&[(0, 120.0), (1, 120.0), (2, 120.0)]));
    }

    // Spec: time-editing R17
    #[test]
    fn inserting_time_in_the_tempo_holds_the_tempo_arriving_there() {
        let mut points = tempo(&[(0, 100.0), (4, 140.0)]);
        insert_tempo_time(&mut points, bar(2), bar(1)).unwrap();
        assert_eq!(
            points,
            tempo(&[(0, 100.0), (2, 120.0), (3, 120.0), (5, 140.0)])
        );
    }

    /// A document with audio and MIDI tracks, their IDs, and the allocator.
    struct Song {
        doc: Document,
        ids: IdAllocator,
        audio: TrackId,
        midi: TrackId,
    }

    impl Song {
        fn new() -> Self {
            let mut ids = IdAllocator::new();
            let main = TrackId(ids.allocate());
            let scenes: Vec<_> = (0..DEFAULT_SCENE_COUNT)
                .map(|_| SceneId(ids.allocate()))
                .collect();
            let mut doc = Document::new(main, scenes);
            let mut track = |kind, index| {
                let id = TrackId(ids.allocate());
                doc.apply(&Command::CreateTrack {
                    id,
                    kind,
                    name: String::new(),
                    parent: None,
                    index,
                })
                .unwrap();
                id
            };
            let (audio, midi) = (track(TrackKind::Audio, 0), track(TrackKind::Midi, 1));
            Self {
                doc,
                ids,
                audio,
                midi,
            }
        }

        fn clip(&mut self, track: TrackId, start: i64, bars: i64) -> ClipId {
            let id = ClipId(self.ids.allocate());
            let data = if track == self.audio {
                ClipData::Audio(AudioData::new(ContentHash([3; 32])))
            } else {
                ClipData::Midi(MidiData::new([]))
            };
            let clip = Clip::new(id, bar(start), bar(bars), data);
            self.doc
                .apply(&Command::CreateClip { track, clip })
                .unwrap();
            id
        }

        fn locator(&mut self, at: i64) -> LocatorId {
            let id = LocatorId(self.ids.allocate());
            let locator = Locator::new(id, bar(at), at.to_string());
            self.doc.apply(&Command::AddLocator { locator }).unwrap();
            id
        }

        /// What `Editor::apply` does before applying: it fills in the IDs
        /// the command leaves out.
        fn apply(&mut self, mut command: Command) -> Result<(), ModelError> {
            let doc = self.doc.clone();
            let ids = &mut self.ids;
            command.issue_missing_ids(&doc, &mut || ids.allocate());
            let result = self.doc.apply(&command);
            self.doc.check_invariants().unwrap();
            result
        }

        fn spans(&self, track: TrackId) -> Vec<(i64, i64)> {
            let mut spans: Vec<_> = self
                .doc
                .track(track)
                .unwrap()
                .clips()
                .map(|c| (c.start.0 / BAR, c.end().0 / BAR))
                .collect();
            spans.sort_unstable();
            spans
        }
    }

    // Spec: time-editing R14, R17
    #[test]
    fn inserting_time_splits_what_it_opens_inside_and_moves_everything_later() {
        let mut song = Song::new();
        let (audio, midi) = (song.audio, song.midi);
        song.clip(audio, 0, 4);
        song.clip(audio, 6, 2);
        song.clip(midi, 2, 1);
        let lane = crate::ids::TakeLaneId(song.ids.allocate());
        let take = ClipId(song.ids.allocate());
        song.apply(Command::AddTakeLane {
            track: audio,
            lane: crate::clip::NewTakeLane {
                id: Some(lane),
                name: "Take 1".to_owned(),
                clips: vec![crate::clip::TakeClip {
                    id: Some(take),
                    start: bar(1),
                    length: bar(2),
                    name: String::new(),
                    data: crate::clip::TakeClipData::Audio(AudioData::new(ContentHash([4; 32]))),
                }],
            },
            index: None,
        })
        .unwrap();
        let loc = song.locator(3);
        song.apply(Command::SetSongStart {
            start: Some(SongStart::Locator(loc)),
        })
        .unwrap();
        song.apply(Command::SetTimeSignatureChange {
            at: bar(5),
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
        let before = song.doc.clone();

        song.apply(Command::InsertTime {
            at: bar(2),
            length: bar(1),
            clips: Vec::new(),
        })
        .unwrap();

        assert_eq!(song.spans(audio), [(0, 2), (3, 5), (7, 9)]);
        assert_eq!(song.spans(midi), [(3, 4)]);
        let locators = song.doc.locators();
        assert_eq!(locators[0].position, bar(4));
        assert_eq!(song.doc.song_start_ticks(), Some(bar(4)));
        let markers: Vec<_> = song.doc.settings().time_signatures.changes().collect();
        assert_eq!(markers[1].at, bar(6));
        // The take lane's clip, bars 1 to 3, is split at 2 as well.
        let lane_spans: Vec<_> = song
            .doc
            .track(audio)
            .unwrap()
            .take_lane(lane)
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.start.0 / BAR, c.end().0 / BAR))
            .collect();
        assert_eq!(lane_spans, [(1, 2), (3, 4)]);
        // The Session View's slots and the loop brace stay as they were.
        assert_eq!(
            song.doc.settings().arrangement_loop,
            before.settings().arrangement_loop
        );
    }

    // Spec: time-editing R15
    #[test]
    fn deleting_time_cuts_or_splits_clips_and_drops_what_lies_inside() {
        let mut song = Song::new();
        let (audio, midi) = (song.audio, song.midi);
        song.clip(audio, 0, 8);
        song.clip(midi, 0, 2);
        song.clip(midi, 3, 1);
        song.clip(midi, 5, 2);
        let inside = song.locator(3);
        let after = song.locator(6);
        song.apply(Command::SetSongStart {
            start: Some(SongStart::Locator(inside)),
        })
        .unwrap();
        song.apply(Command::SetTimeSignatureChange {
            at: bar(3),
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
        song.apply(Command::SetTimeSignatureChange {
            at: bar(7),
            signature: TimeSignature::new(5, 4).unwrap(),
        })
        .unwrap();

        song.apply(Command::DeleteTime {
            start: bar(1),
            end: bar(5),
            clips: Vec::new(),
        })
        .unwrap();

        // The long clip splits, one on each side; the one that crossed the
        // start is cut; the one inside goes; the one after moves up.
        assert_eq!(song.spans(audio), [(0, 1), (1, 4)]);
        assert_eq!(song.spans(midi), [(0, 1), (1, 3)]);
        assert!(song.doc.locator(inside).is_none());
        assert_eq!(song.doc.locator(after).unwrap().position, bar(2));
        assert_eq!(song.doc.song_start_ticks(), None);
        // 3/4 at bar 3 is inside the span: it was in force at the span's
        // end, so it holds from the start. 5/4 at bar 7 moves to bar 3.
        let signatures: Vec<_> = song
            .doc
            .settings()
            .time_signatures
            .changes()
            .map(|c| (c.at, c.signature))
            .collect();
        assert_eq!(
            signatures,
            [
                (Ticks::ZERO, TimeSignature::default()),
                (bar(1), TimeSignature::new(3, 4).unwrap()),
                (bar(3), TimeSignature::new(5, 4).unwrap()),
            ]
        );
    }

    // Spec: time-editing R15, R17
    #[test]
    fn every_envelope_moves_including_the_main_tracks() {
        let mut song = Song::new();
        let main = song.doc.main_id();
        let target = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        song.apply(Command::SetAutomationEnvelope {
            track: main,
            target,
            points: envelope(&[(0, 0.0), (4, 1.0)]).points().collect(),
        })
        .unwrap();
        song.apply(Command::DeleteTime {
            start: bar(1),
            end: bar(3),
            clips: Vec::new(),
        })
        .unwrap();
        let moved = song
            .doc
            .track(main)
            .unwrap()
            .automation_for(&target)
            .unwrap();
        assert_eq!(moved.envelope.points().last().unwrap().at, bar(2));
        song.apply(Command::InsertTime {
            at: bar(1),
            length: bar(2),
            clips: Vec::new(),
        })
        .unwrap();
        let moved = song
            .doc
            .track(main)
            .unwrap()
            .automation_for(&target)
            .unwrap();
        assert_eq!(moved.envelope.points().last().unwrap().at, bar(4));
    }

    #[test]
    fn a_bad_span_or_a_wrong_list_of_new_ids_changes_nothing() {
        let mut song = Song::new();
        let audio = song.audio;
        song.clip(audio, 0, 4);
        let before = song.doc.clone();
        for command in [
            Command::InsertTime {
                at: Ticks(-1),
                length: bar(1),
                clips: Vec::new(),
            },
            Command::InsertTime {
                at: bar(1),
                length: Ticks::ZERO,
                clips: Vec::new(),
            },
            Command::DeleteTime {
                start: bar(2),
                end: bar(2),
                clips: Vec::new(),
            },
            // The clip across 1.2.1 needs a new ID, and none is given.
            Command::InsertTime {
                at: bar(2),
                length: bar(1),
                clips: Vec::new(),
            },
        ] {
            assert!(song.doc.apply(&command).is_err(), "{command:?}");
            assert_eq!(song.doc, before);
        }
        // Left to the editor, the ID is issued.
        song.apply(Command::InsertTime {
            at: bar(2),
            length: bar(1),
            clips: Vec::new(),
        })
        .unwrap();
        assert_eq!(song.spans(audio), [(0, 2), (3, 5)]);
    }

    #[test]
    fn a_time_too_long_to_place_is_refused_and_changes_nothing() {
        let huge = Ticks(i64::MAX - bar(8).0);
        // A tempo breakpoint after the insert and no clip: the shifted
        // breakpoint would wrap.
        let mut song = Song::new();
        song.apply(Command::SetTempoBreakpoint {
            at: bar(9),
            bpm: Bpm::new(140.0).unwrap(),
        })
        .unwrap();
        song.locator(10);
        let before = song.doc.clone();
        let insert = Command::InsertTime {
            at: bar(5),
            length: huge,
            clips: Vec::new(),
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| song.doc.apply(&insert)));
        assert!(matches!(result, Ok(Err(ModelError::PositionOverflow))));
        assert_eq!(song.doc, before);

        // A locator alone would wrap to a negative position.
        let mut song = Song::new();
        song.locator(10);
        let before = song.doc.clone();
        assert!(matches!(
            song.doc.apply(&insert),
            Err(ModelError::PositionOverflow)
        ));
        assert_eq!(song.doc, before);
    }
}

//! The clips of an import (the DAWproject spec, R24 to R37): where each
//! clip starts and how long it is, what it plays, how nested clips flatten,
//! and how clips placed over each other replace what they cover.

use super::*;

/// The most passes a loop is unrolled into or flattened over; a file that
/// asks for more is cut there and says so.
const MAX_PASSES: usize = 4096;

/// How deep clips nest before the rest is left out.
const MAX_DEPTH: usize = 8;

/// The track a clip is made for.
#[derive(Clone, Debug)]
pub(super) struct Lane {
    pub id: TrackId,
    pub kind: TrackKind,
    /// Where the track is, for the report.
    pub place: String,
}

/// Where a clip is and what its content's times count from.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// The unit of the clip's content.
    unit: TimeUnit,
    /// Where the clip starts.
    start: Ticks,
    /// How long it is.
    length: Ticks,
    /// The tick where its content's time zero plays.
    zero: Ticks,
}

/// A clip made, with where it is for the report.
#[derive(Clone, Debug)]
pub(super) struct Piece {
    pub clip: Clip,
    pub place: String,
}

impl Importer<'_, '_> {
    /// The clips of every track (R24 to R26, R37).
    pub(super) fn build_clips(&mut self, project: &DawProject) {
        let Some(arrangement) = &project.arrangement else {
            return;
        };
        // The lanes of each track, in the file's order.
        let mut lanes: Vec<(String, Vec<(model::Clips, TimeUnit)>)> = Vec::new();
        let mut loose: Vec<(String, Timeline, TimeUnit)> = Vec::new();
        for lane in &arrangement.lanes {
            collect_lanes(lane, None, TimeUnit::Beats, &mut lanes, &mut loose);
        }
        // A track's loose timelines are read after its lanes' clips.
        for (id, _, _) in &loose {
            if !lanes.iter().any(|(lane, _)| lane == id) {
                lanes.push((id.clone(), Vec::new()));
            }
        }
        for (track_id, track_lanes) in lanes {
            let Some(track) = self.tracks.get(&track_id).copied() else {
                if self.left_out.contains(&track_id) {
                    continue;
                }
                // A reference to an `id` the file doesn't hold is dropped
                // (R7).
                self.report.add(
                    "lanes for a track the file doesn't hold",
                    "the Arrangement",
                    "left out",
                );
                continue;
            };
            let Some((kind, name)) = self
                .editor
                .document()
                .track(track)
                .map(|track| (track.kind(), track.name().to_owned()))
            else {
                continue;
            };
            let lane = Lane {
                id: track,
                kind,
                place: format!("the track \"{name}\""),
            };
            self.channels_seen.clear();
            // The pieces for each track they land on, in the order they are
            // first made.
            let mut made: Vec<(Lane, Vec<Piece>)> = Vec::new();
            let mut takes: Vec<(String, Vec<Piece>)> = Vec::new();
            for (index, (timeline, unit)) in track_lanes.iter().enumerate() {
                if !kind.holds_clips() {
                    // A group, a return or Main holds no clips (R16, R37).
                    if !timeline.clips.is_empty() {
                        self.report.add_count(
                            &format!("a clip on a {}", kind_word(kind)),
                            &lane.place,
                            "left out",
                            timeline.clips.len(),
                        );
                    }
                    continue;
                }
                if index > 0 && kind == TrackKind::Midi {
                    // MIDI takes wait for WP-6.21 (R25).
                    if !timeline.clips.is_empty() {
                        self.report.add_count(
                            "a further lane of clips",
                            &lane.place,
                            "left out",
                            timeline.clips.len(),
                        );
                    }
                    continue;
                }
                let mut pieces = Vec::new();
                for clip in &timeline.clips {
                    let target = self.lane_for(clip, &lane);
                    let found = self.clips_in(clip, *unit, Ticks::ZERO, &target, 0);
                    if index == 0 {
                        push_pieces(&mut made, target, found);
                    } else {
                        pieces.extend(found);
                    }
                }
                if index > 0 {
                    let name = timeline
                        .info
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Take {index}"));
                    takes.push((name, pieces));
                }
            }
            if kind.holds_clips() {
                for (timeline, unit) in loose
                    .iter()
                    .filter(|(id, _, _)| *id == track_id)
                    .map(|(_, timeline, unit)| (timeline, *unit))
                {
                    let found = self.loose_clip(timeline, unit, &lane);
                    push_pieces(&mut made, lane.clone(), found);
                }
            }
            for (target, mut pieces) in made {
                if target.kind == TrackKind::Midi {
                    self.copy_expressions(&target, &mut pieces);
                }
                self.place_clips(&target, pieces);
            }
            for (name, pieces) in takes {
                self.add_take_lane(&lane, name, pieces);
            }
            if self.channels_seen.len() > 1 {
                // A note's channel isn't kept (R28).
                self.report.add(
                    "notes on more than one channel",
                    &lane.place,
                    "left out; the notes play on one channel",
                );
            }
        }
        self.make_crossfades();
    }

    /// The track a clip belongs on: an audio clip on a MIDI track goes to the
    /// audio track beside it, made when there is none (R37).
    pub(super) fn lane_for(&mut self, source: &model::Clip, lane: &Lane) -> Lane {
        if lane.kind != TrackKind::Midi || !self.content_is_audio(source) {
            return lane.clone();
        }
        let (sibling, made) = match self.audio_sibling(lane.id) {
            Some(sibling) => (sibling, false),
            None => {
                let sibling = self.make_audio_sibling(lane.id);
                self.audio_siblings.push((lane.id, sibling));
                (sibling, true)
            }
        };
        let name = self
            .editor
            .document()
            .track(sibling)
            .map_or_else(String::new, |track| track.name().to_owned());
        if made {
            self.report.add(
                "an audio clip on a MIDI track",
                &lane.place,
                "moved to a new audio track beside it",
            );
        }
        Lane {
            id: sibling,
            kind: TrackKind::Audio,
            place: format!("the track \"{name}\""),
        }
    }

    /// An audio track straight after a MIDI track (R37).
    fn make_audio_sibling(&mut self, midi: TrackId) -> TrackId {
        let document = self.editor.document();
        let name = document
            .track(midi)
            .map_or_else(String::new, |track| track.name().to_owned());
        let parent = document.parent(midi);
        let position = match parent {
            Some(group) => document
                .track(group)
                .and_then(|group| group.children().position(|child| child == midi)),
            None => document.top_level().position(|top| top == midi),
        };
        let index = position.map_or(0, |position| position + 1);
        self.create_track_at(TrackKind::Audio, format!("{name} Audio"), parent, index)
    }

    /// Where a clip is, for the report (R12).
    fn clip_place(&self, source: &model::Clip, start: Ticks, lane: &Lane) -> String {
        let at = match self.scene {
            Some(scene) => format!("in scene {scene}"),
            None => format!("at {}", self.signatures.to_bbs(start)),
        };
        match source.info.name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => format!("the clip \"{name}\" {at} on {}", lane.place),
            None => format!("a clip {at} on {}", lane.place),
        }
    }

    /// The seconds at a tick of the song.
    fn seconds_at(&self, ticks: Ticks) -> f64 {
        self.tempo.ticks_to_seconds(ticks.max(Ticks::ZERO))
    }

    /// How many ticks `value` of `unit` lasts from the tick `from` (R9): a
    /// beat is a quarter note, and a time in seconds is measured through the
    /// tempo map from where it starts.
    pub(super) fn span(&self, unit: TimeUnit, value: f64, from: Ticks) -> Ticks {
        if !value.is_finite() {
            return Ticks::ZERO;
        }
        match unit {
            TimeUnit::Beats => beats_to_ticks(value),
            TimeUnit::Seconds => {
                let from = from.max(Ticks::ZERO);
                let seconds = (self.seconds_at(from) + value).max(0.0);
                self.tempo.seconds_to_ticks(seconds) - from
            }
        }
    }

    /// A marker of an unwarped clip as seconds in its file (R29): a time in
    /// seconds as it is, and one in beats as the seconds those beats last
    /// from the clip's start.
    fn file_seconds(&self, unit: TimeUnit, value: f64, start: Ticks) -> f64 {
        if !value.is_finite() {
            return 0.0;
        }
        match unit {
            TimeUnit::Seconds => value,
            TimeUnit::Beats => {
                self.seconds_at(start + beats_to_ticks(value)) - self.seconds_at(start)
            }
        }
    }

    /// A timeline outside any clip becomes one clip from its start (R24).
    fn loose_clip(&mut self, timeline: &Timeline, unit: TimeUnit, lane: &Lane) -> Vec<Piece> {
        let place = format!("a timeline in {}", lane.place);
        let length = match timeline {
            Timeline::Notes(notes) => {
                // As long as its notes, rounded up to the next bar, as a
                // MIDI file's clip is (the MIDI clips spec, R17).
                let unit = notes.info.time_unit.unwrap_or(unit);
                let end = notes
                    .notes
                    .iter()
                    .filter_map(|note| {
                        let time = note.time?;
                        let duration = note.duration?;
                        (duration > 0.0 && time >= 0.0).then(|| {
                            let start = self.span(unit, time, Ticks::ZERO);
                            start + self.span(unit, duration, start)
                        })
                    })
                    .max()
                    .unwrap_or(Ticks::ZERO);
                let (bar, into) = self.signatures.bar_at(end);
                if into == Ticks::ZERO {
                    end
                } else {
                    self.signatures.bar_start(bar + 1)
                }
            }
            Timeline::Audio(file)
            | Timeline::Warps(model::Warps {
                audio: Some(file), ..
            }) => file.duration.map_or(Ticks::ZERO, |seconds| {
                self.span(TimeUnit::Seconds, seconds, Ticks::ZERO)
            }),
            _ => Ticks::ZERO,
        };
        if length <= Ticks::ZERO {
            self.report
                .add("a timeline with no length", &place, "left out");
            return Vec::new();
        }
        let frame = Frame {
            unit,
            start: Ticks::ZERO,
            length,
            zero: Ticks::ZERO,
        };
        self.content(&model::Clip::default(), timeline, frame, lane, &place, 0)
    }

    /// The pieces a DAWproject clip becomes on `lane` (R27 to R37): `unit`
    /// is its parent timeline's, and `origin` the tick where that
    /// timeline's time zero plays.
    pub(super) fn clips_in(
        &mut self,
        source: &model::Clip,
        unit: TimeUnit,
        origin: Ticks,
        lane: &Lane,
        depth: usize,
    ) -> Vec<Piece> {
        let Some(time) = source.time else {
            // A clip without its time is left out (R4).
            self.report
                .add("a clip without its time", &lane.place, "left out");
            return Vec::new();
        };
        let start = origin + self.span(unit, time, origin);
        let place = self.clip_place(source, start, lane);
        if depth > MAX_DEPTH {
            self.report
                .add("a clip nested too deeply", &place, "left out");
            return Vec::new();
        }
        if source.info.comment.is_some() {
            // Comments aren't kept, and are counted in the report (R20).
            self.report.add("a comment", &place, "left out");
        }
        let content_unit = source.content_time_unit.unwrap_or(unit);
        let Some(content) = self.resolve_content(source, &place) else {
            return Vec::new();
        };
        let play_start = self.span(content_unit, source.play_start.unwrap_or(0.0), start);
        // R27: its duration, else its play region, else its content.
        let length = match (source.duration, source.play_stop) {
            (Some(duration), _) => Some(self.span(unit, duration, start)),
            (None, Some(stop)) => Some(self.span(content_unit, stop, start) - play_start),
            (None, None) => self.content_length(&content, content_unit, start, play_start),
        };
        let Some(length) = length.filter(|length| *length > Ticks::ZERO) else {
            self.report.add("a clip with no length", &place, "left out");
            return Vec::new();
        };
        let frame = Frame {
            unit: content_unit,
            start,
            length,
            zero: start - play_start,
        };
        self.content(source, &content, frame, lane, &place, depth)
    }

    /// What a clip plays: its own content, or a copy of the timeline it
    /// names (R35).
    fn resolve_content(&mut self, source: &model::Clip, place: &str) -> Option<Timeline> {
        if let Some(reference) = &source.reference {
            let Some(timeline) = self.timelines.get(reference.as_str()) else {
                // A reference to something that isn't a timeline leaves the
                // clip out (R7, R35).
                self.report.add(
                    "a clip that names a timeline the file doesn't hold",
                    place,
                    "left out",
                );
                return None;
            };
            self.report.add("an alias clip", place, "read as a copy");
            return Some(timeline.as_ref().clone());
        }
        source
            .content
            .as_ref()
            .and_then(|content| content.timeline())
            .cloned()
    }

    /// How long a clip's content plays when the clip says nothing else
    /// (R27).
    fn content_length(
        &self,
        content: &Timeline,
        unit: TimeUnit,
        start: Ticks,
        play_start: Ticks,
    ) -> Option<Ticks> {
        let end = match content {
            Timeline::Notes(notes) => {
                let unit = notes.info.time_unit.unwrap_or(unit);
                notes
                    .notes
                    .iter()
                    .filter_map(|note| {
                        let at = self.span(unit, note.time?, start);
                        Some(at + self.span(unit, note.duration?, at))
                    })
                    .max()?
            }
            Timeline::Audio(file)
            | Timeline::Warps(model::Warps {
                audio: Some(file), ..
            }) => self.span(TimeUnit::Seconds, file.duration?, start),
            _ => return None,
        };
        Some(end - play_start)
    }

    /// The pieces a clip's content makes (R28 to R37).
    fn content(
        &mut self,
        source: &model::Clip,
        content: &Timeline,
        frame: Frame,
        lane: &Lane,
        place: &str,
        depth: usize,
    ) -> Vec<Piece> {
        match content {
            Timeline::Notes(notes) => self.midi_clip(source, notes, frame, lane, place),
            Timeline::Audio(file) => self.audio_clip(source, file, None, frame, lane, place),
            Timeline::Warps(warps) => match &warps.audio {
                Some(file) => self.audio_clip(source, file, Some(warps), frame, lane, place),
                None => {
                    self.report
                        .add("a warped clip with no file", place, "left out");
                    Vec::new()
                }
            },
            Timeline::Clips(clips) => self.flatten(source, clips, frame, lane, place, depth),
            Timeline::Lanes(lanes) => {
                // A clip of lanes takes its notes or audio from the first of
                // them (R36).
                let mut made = Vec::new();
                let mut found = false;
                let mut points: Vec<&model::Points> = Vec::new();
                for child in &lanes.children {
                    match child {
                        Timeline::Notes(_) | Timeline::Audio(_) | Timeline::Warps(_) => {
                            if found {
                                self.report.add(
                                    "a further timeline in a clip's lanes",
                                    place,
                                    "left out",
                                );
                                continue;
                            }
                            found = true;
                            let frame = Frame {
                                unit: child.info().time_unit.unwrap_or(frame.unit),
                                ..frame
                            };
                            made.extend(self.content(source, child, frame, lane, place, depth + 1));
                        }
                        Timeline::Video(_) => {
                            self.report.add("a video timeline", place, "left out");
                        }
                        Timeline::Points(set) => points.push(set),
                        _ => {}
                    }
                }
                self.lane_points(&points, &mut made, frame, lane, place);
                made
            }
            Timeline::Video(_) => {
                self.report.add("a video clip", place, "left out");
                Vec::new()
            }
            Timeline::Points(_) | Timeline::Markers(_) | Timeline::ClipSlot(_) => {
                self.report
                    .add("a clip that plays no notes or audio", place, "left out");
                Vec::new()
            }
        }
    }

    /// The automation and expressions in a clip's lanes (R59 to R62).
    fn lane_points(
        &mut self,
        points: &[&model::Points],
        made: &mut [Piece],
        frame: Frame,
        lane: &Lane,
        place: &str,
    ) {
        if points.is_empty() || made.is_empty() {
            return;
        }
        let (driving, expressions): (Vec<&model::Points>, Vec<&model::Points>) = points
            .iter()
            .copied()
            .partition(|set| set.target.parameter.is_some());
        if self.scene.is_some() {
            self.session_automation(&driving, &mut made[0].clip, frame.unit, lane, place);
        } else {
            let span = Some((frame.start, frame.start + frame.length));
            for set in &driving {
                if let Some(parameter) = &set.target.parameter {
                    self.add_envelope(parameter, set, frame.unit, frame.zero, span, place);
                }
            }
        }
        if lane.kind == TrackKind::Audio {
            for piece in made.iter_mut() {
                self.audio_expressions(&expressions, &mut piece.clip, place);
            }
            return;
        }
        for set in expressions {
            let steps = self.steps(set, frame.unit, Ticks::ZERO);
            for piece in made.iter_mut() {
                self.expression_into(&mut piece.clip, set, &steps, place);
            }
        }
    }

    /// A MIDI clip (R27, R28).
    fn midi_clip(
        &mut self,
        source: &model::Clip,
        notes: &model::Notes,
        frame: Frame,
        lane: &Lane,
        place: &str,
    ) -> Vec<Piece> {
        if lane.kind != TrackKind::Midi {
            // Notes on an audio track are left out (R37).
            self.report
                .add("notes on an audio track", place, "left out");
            return Vec::new();
        }
        let notes_unit = notes.info.time_unit.unwrap_or(frame.unit);
        let mut made: Vec<Note> = Vec::new();
        let mut left_out = 0usize;
        for note in &notes.notes {
            let (Some(time), Some(duration), Some(key)) = (note.time, note.duration, note.key)
            else {
                self.report
                    .add("a note without its time, length or key", place, "left out");
                continue;
            };
            // A note that can't be played, or starts before its content
            // does, is left out and counted (R28).
            let playable = (0..=127).contains(&key)
                && duration.is_finite()
                && duration > 0.0
                && time.is_finite()
                && time >= 0.0;
            if !playable {
                left_out += 1;
                continue;
            }
            let at = self.span(notes_unit, time, frame.zero);
            let long = self
                .span(notes_unit, duration, frame.zero + at)
                .max(Ticks(1));
            let velocity = note
                .vel
                .map_or(100, |vel| (vel * 127.0).round().clamp(1.0, 127.0) as u8);
            let release = note
                .rel
                .map_or(64, |rel| (rel * 127.0).round().clamp(1.0, 127.0) as u8);
            let mut built = Note::new(key as u8, at, long, velocity);
            built.release_velocity = release;
            if let Some(content) = note.content.as_ref().and_then(|content| content.timeline()) {
                self.note_expressions(&mut built, content, notes_unit, place);
            }
            made.push(built);
            if let Some(channel) = note.channel
                && !self.channels_seen.contains(&channel)
            {
                self.channels_seen.push(channel);
            }
        }
        if left_out > 0 {
            self.report
                .add_count("a note that can't be played", place, "left out", left_out);
        }
        let mut data = MidiData::new(made);
        self.midi_markers(source, &mut data, frame);
        let id = self.editor.new_id();
        let mut clip = Clip::new(id, frame.start, frame.length, ClipData::Midi(data));
        self.finish_clip(&mut clip, source, place);
        vec![Piece {
            clip,
            place: place.to_owned(),
        }]
    }

    /// A MIDI clip's Start and End markers and loop brace (R27).
    fn midi_markers(&self, source: &model::Clip, data: &mut MidiData, frame: Frame) {
        let (unit, start) = (frame.unit, frame.start);
        data.start = self.span(unit, source.play_start.unwrap_or(0.0), start);
        data.end = source
            .play_stop
            .map(|stop| self.span(unit, stop, start))
            .filter(|end| *end > data.start);
        if let (Some(from), Some(to)) = (source.loop_start, source.loop_end) {
            let (from, to) = (self.span(unit, from, start), self.span(unit, to, start));
            if to > from && data.start < to {
                data.looping = true;
                data.loop_start = from.max(Ticks::ZERO);
                data.loop_end = Some(to);
            }
        }
    }

    /// An audio clip, warped when `warps` holds events that rise (R29 to
    /// R32).
    fn audio_clip(
        &mut self,
        source: &model::Clip,
        file: &MediaFile,
        warps: Option<&model::Warps>,
        frame: Frame,
        lane: &Lane,
        place: &str,
    ) -> Vec<Piece> {
        if lane.kind != TrackKind::Audio {
            self.report
                .add("an audio clip on a MIDI track", place, "left out");
            return Vec::new();
        }
        let Some(hash) = self.media_for(file, place) else {
            return Vec::new();
        };
        let mut data = AudioData::new(hash);
        let mut warped = false;
        if let Some(warps) = warps {
            match self.warp_markers(warps, frame) {
                Some(markers) => {
                    warped = true;
                    self.warped_data(&mut data, source, file, markers, frame, place);
                }
                None => self.report.add(
                    "a warped clip whose warps don't rise",
                    place,
                    "read as an unwarped clip",
                ),
            }
        }
        let id = self.editor.new_id();
        let mut clip = Clip::new(id, frame.start, frame.length, ClipData::Audio(data));
        self.finish_clip(&mut clip, source, place);
        let mut pieces = if warped {
            vec![Piece {
                clip,
                place: place.to_owned(),
            }]
        } else {
            self.unwarped_data(&mut clip, source, frame);
            self.unroll(clip, place)
        };
        self.apply_fades(&mut pieces, source, frame, lane);
        pieces
    }

    /// The seconds `value` of `unit` lasts from `at`, forwards or backwards
    /// (R33): a beat is a quarter note, and the tempo map says how long.
    fn fade_seconds(&self, unit: TimeUnit, value: f64, at: Ticks, forwards: bool) -> f64 {
        if !value.is_finite() {
            return 0.0;
        }
        match unit {
            TimeUnit::Seconds => value,
            TimeUnit::Beats => {
                let beats = beats_to_ticks(value);
                if forwards {
                    self.seconds_at(at + beats) - self.seconds_at(at)
                } else {
                    self.seconds_at(at) - self.seconds_at(at - beats)
                }
            }
        }
    }

    /// The fades of a clip, in seconds, each limited to the clip's length: a
    /// positive fade-in and a fade-out go to the pieces at the clip's edges,
    /// and a negative fade-in is a crossfade with the clip before it
    /// (R33).
    fn apply_fades(
        &mut self,
        pieces: &mut [Piece],
        source: &model::Clip,
        frame: Frame,
        lane: &Lane,
    ) {
        let unit = source.fade_time_unit.unwrap_or(frame.unit);
        let end = frame.start + frame.length;
        if let Some(value) = source.fade_in
            && value < 0.0
        {
            let seconds = self.fade_seconds(unit, -value, frame.start, true);
            if seconds > 0.0 && self.scene.is_none() {
                self.crossfades.push((lane.id, frame.start, seconds));
            }
        }
        for piece in pieces.iter_mut() {
            let (start, stop) = (piece.clip.start, piece.clip.end());
            let length = self.seconds_at(stop) - self.seconds_at(start);
            let ClipData::Audio(data) = &mut piece.clip.data else {
                continue;
            };
            let mut cut = false;
            if start == frame.start
                && let Some(value) = source.fade_in.filter(|value| *value > 0.0)
            {
                let seconds = self.fade_seconds(unit, value, start, true);
                data.fade_in = seconds.min(length);
                cut |= seconds > length;
            }
            if stop == end
                && let Some(value) = source.fade_out.filter(|value| *value > 0.0)
            {
                let seconds = self.fade_seconds(unit, value, stop, false);
                // A fade-out ends no earlier than the fade-in does (R25).
                let room = (length - data.fade_in).max(0.0);
                data.fade_out = seconds.min(room);
                cut |= seconds > room;
            }
            if cut {
                self.report.add(
                    "a fade longer than its clip",
                    &piece.place,
                    "shortened to the clip",
                );
            }
        }
    }

    /// The warp markers of a `Warps` timeline, when its events rise in both
    /// times and its file times are seconds (R30).
    fn warp_markers(&self, warps: &model::Warps, frame: Frame) -> Option<Vec<WarpMarker>> {
        if warps.content_time_unit.unwrap_or(TimeUnit::Seconds) != TimeUnit::Seconds {
            return None;
        }
        let events_unit = warps.info.time_unit.unwrap_or(frame.unit);
        let mut markers: Vec<WarpMarker> = Vec::new();
        for warp in &warps.warps {
            let (Some(time), Some(content_time)) = (warp.time, warp.content_time) else {
                return None;
            };
            let marker = WarpMarker {
                time: content_time,
                beat: self.span(events_unit, time, frame.start),
            };
            if !content_time.is_finite() || content_time < 0.0 {
                return None;
            }
            if let Some(last) = markers.last()
                && (marker.beat <= last.beat || marker.time <= last.time)
            {
                return None;
            }
            markers.push(marker);
        }
        (markers.len() >= 2).then_some(markers)
    }

    /// A warped clip's data: warp markers, tempo, mode, markers and loop
    /// brace in ticks of its contents (R30, R31).
    fn warped_data(
        &mut self,
        data: &mut AudioData,
        source: &model::Clip,
        file: &MediaFile,
        markers: Vec<WarpMarker>,
        frame: Frame,
        place: &str,
    ) {
        data.warp = true;
        // The tempo of the last segment (R30).
        data.tempo = markers.windows(2).last().map(|pair| {
            let beats = (pair[1].beat - pair[0].beat).as_quarters();
            let seconds = pair[1].time - pair[0].time;
            (beats / seconds * 60.0).clamp(Bpm::MIN.get(), Bpm::MAX.get())
        });
        data.warp_markers = markers;
        data.warp_mode = self.warp_mode(file.algorithm.as_deref(), place);
        let ticks = |value: f64| self.span(frame.unit, value, frame.start).get() as f64;
        data.start = ticks(source.play_start.unwrap_or(0.0));
        data.end = source.play_stop.map(ticks).filter(|end| *end > data.start);
        if let (Some(from), Some(to)) = (source.loop_start, source.loop_end) {
            let (from, to) = (ticks(from), ticks(to));
            if to > from && data.start < to {
                data.looping = true;
                data.loop_start = from;
                data.loop_end = Some(to);
            }
        }
    }

    /// An unwarped clip's Start and End markers and loop brace, seconds in
    /// its file (R29).
    fn unwarped_data(&self, clip: &mut Clip, source: &model::Clip, frame: Frame) {
        let ClipData::Audio(data) = &mut clip.data else {
            return;
        };
        let seconds = |value: f64| self.file_seconds(frame.unit, value, frame.start);
        data.start = seconds(source.play_start.unwrap_or(0.0)).max(0.0);
        data.end = source
            .play_stop
            .map(seconds)
            .filter(|end| *end > data.start);
        if let (Some(from), Some(to)) = (source.loop_start, source.loop_end) {
            let (from, to) = (seconds(from).max(0.0), seconds(to));
            if to > from && data.start < to {
                data.looping = true;
                data.loop_start = from;
                data.loop_end = Some(to);
            }
        }
    }

    /// An unwarped clip in the Arrangement whose loop would play more than
    /// once in its length becomes one unlooped clip per pass (R32).
    fn unroll(&mut self, clip: Clip, place: &str) -> Vec<Piece> {
        let ClipData::Audio(data) = &clip.data else {
            return Vec::new();
        };
        let single = vec![Piece {
            clip: clip.clone(),
            place: place.to_owned(),
        }];
        let (true, Some(loop_end)) = (data.looping, data.loop_end) else {
            return single;
        };
        let first = loop_end - data.start;
        let brace = loop_end - data.loop_start;
        let begin = self.seconds_at(clip.start);
        let end = clip.end();
        let reaches = self.seconds_at(end) - begin;
        // A Session clip keeps its loop; so does one that never wraps.
        if self.scene.is_some() || reaches <= first + 1e-6 || brace < 1e-3 {
            return single;
        }
        let mut pieces = Vec::new();
        let mut from = clip.start;
        let mut offset = first;
        let mut n = 0usize;
        while from < end && n < MAX_PASSES {
            let to = if n == 0 {
                self.tempo.seconds_to_ticks(begin + first)
            } else {
                self.tempo.seconds_to_ticks(begin + offset)
            };
            let to = to.min(end).max(from + Ticks(1));
            let mut part = clip.clone();
            part.id = self.editor.new_id();
            part.start = from;
            part.length = to - from;
            if let ClipData::Audio(audio) = &mut part.data {
                audio.looping = false;
                audio.loop_start = 0.0;
                audio.loop_end = None;
                if n == 0 {
                    audio.end = Some(loop_end);
                } else {
                    audio.start = data.loop_start;
                    audio.end = Some(loop_end);
                }
            }
            pieces.push(Piece {
                clip: part,
                place: place.to_owned(),
            });
            from = to;
            offset += brace;
            n += 1;
        }
        if from < end {
            self.report.add(
                "a loop that runs more than 4096 passes",
                place,
                "cut after the 4096th pass",
            );
        }
        self.report.add(
            "a loop in an unwarped clip",
            place,
            &format!("unrolled into {} clips", pieces.len()),
        );
        pieces
    }

    /// A warped clip's mode (R31).
    fn warp_mode(&mut self, algorithm: Option<&str>, place: &str) -> WarpMode {
        let Some(algorithm) = algorithm else {
            return self.default_warp_mode;
        };
        let name: String = algorithm
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        match name.as_str() {
            "repitch" => WarpMode::RePitch,
            "beats" => WarpMode::Beats,
            "tones" => WarpMode::Tones,
            "texture" => WarpMode::Texture,
            "complex" => WarpMode::Complex,
            "complexpro" => WarpMode::ComplexPro,
            _ => {
                // Any other name takes the Default Warp Mode (R31).
                self.report.add(
                    &format!("the warp mode `{algorithm}`"),
                    place,
                    "changed to the default warp mode",
                );
                self.default_warp_mode
            }
        }
    }

    /// A clip whose content is a timeline of clips is flattened (R34).
    fn flatten(
        &mut self,
        source: &model::Clip,
        clips: &model::Clips,
        frame: Frame,
        lane: &Lane,
        place: &str,
        depth: usize,
    ) -> Vec<Piece> {
        // The inner clips, with the content's time zero at `frame.zero`.
        let inner_unit = clips.info.time_unit.unwrap_or(frame.unit);
        let mut inner = Vec::new();
        for clip in &clips.clips {
            inner.extend(self.clips_in(clip, inner_unit, frame.zero, lane, depth + 1));
        }
        if inner.is_empty() {
            return Vec::new();
        }
        self.report
            .add_count("a nested clip", place, "flattened", inner.len());
        if lane.kind == TrackKind::Midi {
            return self.merge_notes(source, inner, frame, place);
        }
        let Frame {
            unit,
            start,
            length,
            zero,
        } = frame;
        // Each pass of the outer clip's loop plays a region of the content:
        // the first from the Start marker, the rest the loop brace.
        let end = start + length;
        let play_start = start - zero;
        let brace = match (source.loop_start, source.loop_end) {
            (Some(from), Some(to)) => {
                let (from, to) = (self.span(unit, from, start), self.span(unit, to, start));
                (to > from && play_start < to).then_some((from, to))
            }
            _ => None,
        };
        // (from, to, shift) in the song's ticks.
        let first_end = brace.map_or(end, |(_, to)| end.min(zero + to));
        let mut passes = vec![(start, first_end, Ticks::ZERO)];
        if let Some((from, to)) = brace {
            let mut at = zero + to;
            while at < end && passes.len() < MAX_PASSES {
                passes.push((at, end.min(at + (to - from)), at - (zero + from)));
                at += to - from;
            }
            if at < end {
                self.report.add(
                    "a loop that runs more than 4096 passes",
                    place,
                    "cut after the 4096th pass",
                );
            }
        }
        let tempo = self.tempo.clone();
        let mut made = Vec::new();
        for (from, to, shift) in passes {
            for piece in &inner {
                let mut moved = piece.clone();
                moved.clip.start += shift;
                let (a, b) = (moved.clip.start.max(from), moved.clip.end().min(to));
                if a >= b {
                    continue;
                }
                let mut part = moved.clip.part(a, b, &tempo);
                part.id = self.editor.new_id();
                made.push(Piece {
                    clip: part,
                    place: moved.place,
                });
            }
        }
        // The outer clip's fades go to the pieces at its edges (R33).
        self.apply_fades(&mut made, source, frame, lane);
        made
    }

    /// The inner clips of a MIDI clip merged into one with the outer clip's
    /// place, markers and loop (R34).
    fn merge_notes(
        &mut self,
        source: &model::Clip,
        inner: Vec<Piece>,
        frame: Frame,
        place: &str,
    ) -> Vec<Piece> {
        let mut notes = Vec::new();
        for piece in inner {
            let ClipData::Midi(data) = &piece.clip.data else {
                continue;
            };
            // Each note as the inner clip plays it, cut to its region.
            let region = piece.clip.length;
            let mut at = Ticks::ZERO;
            let mut from = data.start;
            let mut to = data.loop_end.filter(|_| data.looping).or(data.end);
            for _ in 0..MAX_PASSES {
                let take = to.map_or(region - at, |to| (to - from).min(region - at));
                if take <= Ticks::ZERO {
                    break;
                }
                for note in data.notes.iter() {
                    if note.start < from || note.start >= from + take {
                        continue;
                    }
                    let mut note = note.clone();
                    note.length = note.length.min(from + take - note.start);
                    note.start = piece.clip.start - frame.zero + at + (note.start - from);
                    notes.push(note);
                }
                at += take;
                if !data.looping || at >= region {
                    break;
                }
                let Some(end) = data.loop_end else { break };
                (from, to) = (data.loop_start, Some(end));
            }
        }
        let mut data = MidiData::new(notes);
        self.midi_markers(source, &mut data, frame);
        let id = self.editor.new_id();
        let mut clip = Clip::new(id, frame.start, frame.length, ClipData::Midi(data));
        self.finish_clip(&mut clip, source, place);
        vec![Piece {
            clip,
            place: place.to_owned(),
        }]
    }

    /// Places a track's clips in order of their start, then the file's
    /// order, each replacing what it covers of those before it (R26).
    fn place_clips(&mut self, lane: &Lane, mut pieces: Vec<Piece>) {
        pieces.sort_by_key(|piece| piece.clip.start);
        let tempo = self.tempo.clone();
        let mut placed: Vec<Piece> = Vec::new();
        for piece in pieces {
            let (start, end) = (piece.clip.start, piece.clip.end());
            let mut kept = Vec::new();
            for earlier in placed {
                let (from, to) = (earlier.clip.start, earlier.clip.end());
                if to <= start || from >= end {
                    kept.push(earlier);
                    continue;
                }
                let mut cut = |piece: &Piece, a: Ticks, b: Ticks| {
                    let mut part = piece.clip.part(a, b, &tempo);
                    part.id = self.editor.new_id();
                    Piece {
                        clip: part,
                        place: piece.place.clone(),
                    }
                };
                let did = match (from < start, to > end) {
                    (true, true) => {
                        let (left, right) = (cut(&earlier, from, start), cut(&earlier, end, to));
                        kept.extend([left, right]);
                        "split"
                    }
                    (true, false) => {
                        kept.push(cut(&earlier, from, start));
                        "shortened"
                    }
                    (false, true) => {
                        kept.push(cut(&earlier, end, to));
                        "shortened"
                    }
                    (false, false) => "removed",
                };
                self.report
                    .add("a clip another clip covers", &earlier.place, did);
            }
            kept.push(piece);
            placed = kept;
        }
        placed.sort_by_key(|piece| piece.clip.start);
        for Piece { clip, place } in placed {
            let command = bl_model::Command::CreateClip {
                track: lane.id,
                clip,
            };
            if let Err(error) = self.editor.apply(command) {
                self.report.add(
                    "a clip Beatlayer can't hold",
                    &place,
                    &format!("left out; {error}"),
                );
            }
        }
    }

    /// A take lane under an audio track (R25).
    fn add_take_lane(&mut self, lane: &Lane, name: String, pieces: Vec<Piece>) {
        let clips: Vec<bl_model::TakeClip> = pieces
            .into_iter()
            .filter_map(|Piece { clip, .. }| match clip.data {
                ClipData::Audio(data) => Some(bl_model::TakeClip {
                    id: None,
                    name: clip.name,
                    start: clip.start,
                    length: clip.length,
                    data: bl_model::TakeClipData::Audio(data),
                }),
                ClipData::Midi(_) => None,
            })
            .collect();
        if clips.is_empty() {
            return;
        }
        let command = bl_model::Command::AddTakeLane {
            track: lane.id,
            lane: bl_model::NewTakeLane {
                id: None,
                name,
                clips,
            },
            index: None,
        };
        if let Err(error) = self.editor.apply(command) {
            self.report.add(
                "a lane of clips Beatlayer can't hold",
                &lane.place,
                &format!("left out; {error}"),
            );
        }
    }
}

impl Importer<'_, '_> {
    /// Makes the crossfades the clips of a track asked for with a negative
    /// fade-in (R33), as `arrangement.set_crossfade` does: the two clips
    /// meet where the later starts, and the crossfade is as long as the
    /// negative value, shared by the boundary and shortened to what the two
    /// clips allow.
    fn make_crossfades(&mut self) {
        for (track, boundary, seconds) in std::mem::take(&mut self.crossfades) {
            let place = self.editor.document().track(track).map_or_else(
                || "a track".to_owned(),
                |held| format!("the track \"{}\"", held.name()),
            );
            let document = self.editor.document();
            let meets = document.track(track).is_some_and(|held| {
                held.clips().any(|clip| clip.end() == boundary)
                    && held.clips().any(|clip| clip.start == boundary)
            });
            if !meets {
                self.report
                    .add("a crossfade with no clip before it", &place, "left out");
                continue;
            }
            // The whole crossfade, half of it either side, then all of it
            // after the boundary, and then shorter ones, until the clips
            // allow one.
            let mut length = seconds;
            let made = loop {
                let tries = [(length / 2.0, length / 2.0), (0.0, length)];
                let made = tries.iter().any(|&(before, after)| {
                    self.editor
                        .apply(bl_model::Command::SetCrossfade {
                            track,
                            boundary,
                            before,
                            after,
                            curve: 0.0,
                        })
                        .is_ok()
                });
                if made || length < 0.001 {
                    break made;
                }
                length /= 2.0;
            };
            if !made {
                self.report
                    .add("a crossfade the clips don't allow", &place, "left out");
            } else if length < seconds {
                self.report.add(
                    "a crossfade longer than its clips allow",
                    &place,
                    "shortened",
                );
            }
        }
    }
}

/// Adds pieces to the track they land on, keeping the order tracks are first
/// used.
fn push_pieces(made: &mut Vec<(Lane, Vec<Piece>)>, lane: Lane, pieces: Vec<Piece>) {
    match made.iter_mut().find(|(known, _)| known.id == lane.id) {
        Some((_, existing)) => existing.extend(pieces),
        None => made.push((lane, pieces)),
    }
}

impl Importer<'_, '_> {
    /// A clip's name, switch and colour (R23, R27).
    fn finish_clip(&mut self, clip: &mut Clip, source: &model::Clip, place: &str) {
        if let Some(name) = &source.info.name {
            clip.name = name.clone();
        }
        if source.enable == Some(false) {
            clip.mute = true;
        }
        self.colour_clip(clip, source.info.color.as_deref(), place);
    }
}

/// What a track that holds no clips is called, for the report.
fn kind_word(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Return => "return",
        TrackKind::Main => "master channel",
        _ => "group",
    }
}

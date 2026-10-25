//! Bouncing tracks and pasting bounced audio (WP-4.15): the render goes
//! through the export's offline engine and the files through WP-4.20's
//! processed-file writer, and the edit lands as one undo step when every
//! file is whole. The behaviour is in the freeze and bounce spec, R18 to
//! R26 and R27 to R29.

use std::cell::RefCell;
use std::rc::Rc;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, AutomationTarget, BouncedAutomation, BouncedTrack, Clip, ClipData, ClipId, Command,
    ContentHash, Document, Editor, FrozenClip, FrozenTrack, MidiData, NewTakeLane, SceneId,
    TakeLaneId, Track, TrackId, TrackKind,
};
use bl_shell_spec::bounce::{
    self as ids, BounceInPlaceArgs, BounceResult, BounceToNewTrackArgs, Channels,
    PasteBouncedAudioArgs,
};
use bl_shell_spec::editing::TargetArgs;
use serde_json::Value;

use super::freeze::{frames_to_ticks, prepared_track, render_doc, seconds_between};
use super::*;
use crate::arrangement::editing::{self, Clipboard, Target};
use crate::arrangement::placing;
use crate::processed::{Kind, Request, Written};

/// The channels a render has: the engine's own, two.
const RENDER_CHANNELS: u16 = 2;

/// A frame at or below this is silence for a tail's end: −120 dBFS.
const FLOOR: f32 = 1e-6;

impl Shell {
    /// Runs one of the bounce commands (WP-4.15).
    pub(super) fn run_bounce(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::BOUNCE_IN_PLACE => {
                let args: BounceInPlaceArgs = Self::args(id, args)?;
                self.bounce_in_place(args)
            }
            ids::BOUNCE_TO_NEW_TRACK => {
                let args: BounceToNewTrackArgs = Self::args(id, args)?;
                self.bounce_to_new_track(args)
            }
            ids::PASTE_BOUNCED_AUDIO => {
                let args: PasteBouncedAudioArgs = Self::args(id, args)?;
                self.paste_bounced_audio(args)
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether a bounce command can run with `args`: the menus' enabled
    /// state (R28).
    pub(super) fn can_bounce(&self, id: &str, args: &Value) -> bool {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        match id {
            ids::BOUNCE_IN_PLACE => {
                let Ok(args) = Self::args::<BounceInPlaceArgs>(id, args) else {
                    return false;
                };
                self.bounce_targets(args.tracks.as_deref())
                    .iter()
                    .any(|track| self.can_bounce_track(*track))
            }
            ids::BOUNCE_TO_NEW_TRACK => {
                let Ok(args) = Self::args::<BounceToNewTrackArgs>(id, args) else {
                    return false;
                };
                self.bounce_target(&args)
                    .map(|target| {
                        editing::material(self.editor.document(), &target)
                            .0
                            .iter()
                            .any(|(_, clips)| clips.iter().any(|clip| !clip.mute))
                    })
                    .unwrap_or(false)
            }
            ids::PASTE_BOUNCED_AUDIO => {
                let Ok(args) = Self::args::<PasteBouncedAudioArgs>(id, args) else {
                    return false;
                };
                self.clipboard
                    .as_ref()
                    .and_then(Clipboard::material)
                    .is_some_and(|copied| !copied.lanes.is_empty())
                    && self.paste_destination(&args).is_ok()
            }
            _ => false,
        }
    }

    /// Whether a track can be bounced: it holds clips, or it is a group
    /// with tracks in it (R21, R23).
    fn can_bounce_track(&self, track: TrackId) -> bool {
        self.editor
            .document()
            .track(track)
            .is_some_and(|track| match track.kind() {
                TrackKind::Audio | TrackKind::Midi => {
                    track.clips().next().is_some() || track.slots().next().is_some()
                }
                TrackKind::Group => track.children().next().is_some(),
                _ => false,
            })
    }

    /// The tracks a bounce acts on: those named, or the selected tracks.
    fn bounce_targets(&self, tracks: Option<&[TrackId]>) -> Vec<TrackId> {
        match tracks {
            Some(tracks) => tracks.to_vec(),
            None => self.arrangement.selected_tracks().to_vec(),
        }
    }

    /// `track.bounce_in_place` (R21, R23).
    fn bounce_in_place(&mut self, args: BounceInPlaceArgs) -> Result<Value, ShellError> {
        let id = ids::BOUNCE_IN_PLACE;
        let tracks = self.bounce_targets(args.tracks.as_deref());
        if tracks.is_empty() {
            return Err(unavailable(id, "no track is selected"));
        }
        let mut files = Vec::new();
        let mut bounces = Vec::new();
        for track in tracks {
            let bounced = self.bounce_track(id, track, args.channels, &mut files)?;
            bounces.push((track, bounced));
        }
        let made: Vec<ClipId> = bounces
            .iter()
            .flat_map(|(_, bounced)| bounced.clip_ids())
            .collect();
        let look = &self.prefs.look;
        self.editor.transact("Bounce tracks in place", |editor| {
            for (track, bounced) in &bounces {
                editor
                    .apply(Command::BounceTrackInPlace {
                        track: *track,
                        bounced: bounced.clone(),
                    })
                    .map_err(|error| failed(id, error.to_string()))?;
            }
            crate::colours::colour_new(editor, look, &[], &made)
                .map_err(|error| failed(id, error.to_string()))
        })?;
        self.changed();
        Ok(to_value(&BounceResult {
            tracks: bounces.iter().map(|(track, _)| *track).collect(),
            clips: made,
            files,
        }))
    }

    /// Renders `track` and writes its bounce file, returning what the new
    /// track holds (R18 to R24).
    fn bounce_track(
        &mut self,
        id: &str,
        track: TrackId,
        channels: Channels,
        files: &mut Vec<String>,
    ) -> Result<BouncedTrack, ShellError> {
        let doc = self.editor.document().clone();
        let held = doc
            .track(track)
            .ok_or_else(|| failed(id, "no such track"))?;
        match held.kind() {
            TrackKind::Audio | TrackKind::Midi => {
                let (clips, slots) = self.bounce_lane(id, track, channels, true, files)?;
                Ok(BouncedTrack {
                    name: bounced_name(held.name()),
                    mixer: bounced_mixer(held),
                    sends: held.sends().collect(),
                    output: held.routing().output,
                    automation: mixer_automation(held),
                    clips,
                    slots,
                })
            }
            TrackKind::Group => {
                // R23: the group's mixer is in the audio, so the new track's
                // mixer stays at rest.
                let end = arrangement_end(&doc);
                self.bounce_group(id, track, channels, Ticks::ZERO, end, files)
            }
            _ => Err(unavailable(id, "a return track can't be bounced")),
        }
    }

    /// Renders a track's Arrangement lane and each of its Session clips,
    /// and returns the bounced clips (R18, R21).
    fn bounce_lane(
        &mut self,
        id: &str,
        track: TrackId,
        channels: Channels,
        keep_mute: bool,
        files: &mut Vec<String>,
    ) -> Result<Bounced, ShellError> {
        let doc = self.editor.document().clone();
        let held = doc
            .track(track)
            .ok_or_else(|| failed(id, "no such track"))?;
        let mut clips: Vec<Clip> = held.clips().cloned().collect();
        clips.sort_by_key(|clip| (clip.start, clip.id));
        let mut lane = Vec::new();
        if !clips.is_empty() {
            let (start, end) = arrangement_bounds(&clips);
            lane = self.bounce_clips(id, track, &clips, start, end, channels, keep_mute, files)?;
        }
        let mut slots = Vec::new();
        for (scene, clip) in held.slots() {
            let (media, start_sec, end_sec, cycle) =
                self.bounce_session_clip(id, track, scene, clip, channels, files)?;
            let region = FrozenClip {
                clip: clip.id,
                media,
                start: start_sec,
                end: end_sec,
                tempo: doc.settings().tempo.bpm_at(Ticks::ZERO).get(),
                length: clip.length,
                original: ClipData::Midi(MidiData::default()),
                scene: Some(scene),
                cycle,
            };
            slots.push((
                scene,
                Clip {
                    id: self.editor.new_id(),
                    name: bounced_name(&clip.name),
                    start: Ticks::ZERO,
                    length: clip.length,
                    offset: Ticks::ZERO,
                    mute: false,
                    data: FrozenTrack::default().audio_for(&region),
                    envelopes: Vec::new(),
                    launch: clip.launch,
                    // Spec: scales R7. A bounced clip keeps its source's
                    // scale.
                    scale: clip.scale,
                    // A bounce plays the groove, so the audio holds it already.
                    groove: None,
                    colour: None,
                },
            ));
        }
        Ok((lane, slots))
    }

    /// Renders `clips` from `start` to `end` and returns the bounced clips
    /// and the file they play (R18).
    #[expect(
        clippy::too_many_arguments,
        reason = "the command, the track, the clips, the span, the channels, and the files"
    )]
    fn bounce_clips(
        &mut self,
        id: &str,
        track: TrackId,
        clips: &[Clip],
        start: Ticks,
        end: Ticks,
        channels: Channels,
        keep_mute: bool,
        files: &mut Vec<String>,
    ) -> Result<Vec<Clip>, ShellError> {
        let doc = self.editor.document().clone();
        let (frames, rate) = self.render_bounce(id, track, start, end, true)?;
        let total = frames.len() as f64 / f64::from(RENDER_CHANNELS) / f64::from(rate);
        let tempo = doc.settings().tempo.bpm_at(start).get();
        // R18: a clip's tail becomes part of the bounced clip, which grows
        // to hold it, up to the next clip's start.
        let spans = clip_spans(&doc, clips, start, &frames, rate, total);
        let label = crate::routing::track_label(&doc, track);
        let media = self.write_bounce(
            id,
            &format!("{label} Bounce [{}].wav", crate::recording::stamp()),
            frames,
            rate,
            channels,
            files,
        )?;
        let mut lane = Vec::with_capacity(clips.len());
        for (index, clip) in clips.iter().enumerate() {
            let (from, end, length) = spans[index];
            let region = FrozenClip {
                clip: clip.id,
                media,
                start: from,
                end,
                tempo,
                length,
                original: ClipData::Midi(MidiData::default()),
                scene: None,
                cycle: None,
            };
            lane.push(Clip {
                id: self.editor.new_id(),
                name: bounced_name(&clip.name),
                start: clip.start,
                length,
                offset: Ticks::ZERO,
                mute: keep_mute && clip.mute,
                data: FrozenTrack::default().audio_for(&region),
                envelopes: Vec::new(),
                launch: clip.launch,
                // Spec: scales R7. A bounced clip keeps its source's scale.
                scale: clip.scale,
                // A bounce plays the groove, so the audio holds it already.
                groove: None,
                colour: None,
            });
        }
        Ok(lane)
    }

    /// Renders a group's output, after its mixer, and returns the clip
    /// that plays it (R23, R24).
    fn bounce_group(
        &mut self,
        id: &str,
        group: TrackId,
        channels: Channels,
        start: Ticks,
        end: Ticks,
        files: &mut Vec<String>,
    ) -> Result<BouncedTrack, ShellError> {
        let doc = self.editor.document().clone();
        if end <= start {
            return Err(unavailable(id, "the group holds no clips"));
        }
        let prepared = prepared_group(&doc, group).map_err(|why| failed(id, why))?;
        let (frames, rate) = render_doc(self, id, &prepared, start, end, false)?;
        let total = frames.len() as f64 / f64::from(RENDER_CHANNELS) / f64::from(rate);
        let label = crate::routing::track_label(&doc, group);
        let media = self.write_bounce(
            id,
            &format!("{label} Bounce [{}].wav", crate::recording::stamp()),
            frames,
            rate,
            channels,
            files,
        )?;
        let tempo = doc.settings().tempo.bpm_at(start).get();
        let mut audio = AudioData::new(media);
        audio.end = Some(total);
        audio.tempo = Some(tempo);
        let name = doc.track(group).map_or("", |track| track.name());
        let clip = Clip {
            id: self.editor.new_id(),
            name: bounced_name(name),
            start,
            length: end - start,
            offset: Ticks::ZERO,
            mute: false,
            data: ClipData::Audio(audio),
            envelopes: Vec::new(),
            launch: Default::default(),
            // Spec: scales R7. A clip made from nothing takes the set's
            // current scale.
            scale: doc.settings().scale,
            // A bounce plays the groove, so the audio holds it already.
            groove: None,
            colour: None,
        };
        Ok(BouncedTrack {
            name: bounced_name(name),
            // R23: the group's mixer is in the audio, so the new track's
            // mixer stays at rest.
            mixer: Default::default(),
            sends: Vec::new(),
            output: doc
                .track(group)
                .map_or(bl_model::Output::Default, |track| track.routing().output),
            automation: Vec::new(),
            clips: vec![clip],
            slots: Vec::new(),
        })
    }

    /// Renders a Session clip alone, from its Start marker to the end of
    /// its second turn round the loop (R4), and writes its file.
    fn bounce_session_clip(
        &mut self,
        id: &str,
        track: TrackId,
        _scene: SceneId,
        clip: &Clip,
        channels: Channels,
        files: &mut Vec<String>,
    ) -> Result<(ContentHash, f64, f64, Option<f64>), ShellError> {
        let doc = self.editor.document().clone();
        let mut prepared = prepared_track(&doc, track).map_err(|why| failed(id, why))?;
        // The track plays only this clip, from 1.1.1: its Arrangement clips
        // and the other slots come out.
        let others: Vec<ClipId> = prepared
            .track(track)
            .map(|track| {
                track
                    .clips()
                    .map(|clip| clip.id)
                    .chain(track.slots().map(|(_, clip)| clip.id))
                    .filter(|id| *id != clip.id)
                    .collect()
            })
            .unwrap_or_default();
        for other in others {
            prepared
                .apply(&Command::DeleteClip { clip: other })
                .map_err(|error| failed(id, error.to_string()))?;
        }
        let mut played = clip.clone();
        played.start = Ticks::ZERO;
        if prepared
            .track(track)
            .and_then(|track| track.slots().next())
            .is_some()
        {
            prepared
                .apply(&Command::DeleteClip { clip: clip.id })
                .map_err(|error| failed(id, error.to_string()))?;
            let copy = self.editor.new_id();
            played.id = copy;
            prepared
                .apply(&Command::CreateClip {
                    track,
                    clip: played,
                })
                .map_err(|error| failed(id, error.to_string()))?;
        }
        let turns = if clip.data.is_audio()
            && matches!(&clip.data, ClipData::Audio(audio) if audio.looping)
        {
            2
        } else {
            1
        };
        let length = clip.length * turns;
        let (frames, rate) = render_doc(self, id, &prepared, Ticks::ZERO, length, false)?;
        let seconds = frames.len() as f64 / f64::from(RENDER_CHANNELS) / f64::from(rate);
        let label = crate::routing::track_label(&doc, track);
        let cycle =
            (turns == 2).then(|| seconds_between(&doc, Ticks::ZERO, Ticks::ZERO + clip.length));
        let media = self.write_bounce(
            id,
            &format!("{label} Bounce [{}].wav", crate::recording::stamp()),
            frames,
            rate,
            channels,
            files,
        )?;
        Ok((media, 0.0, seconds, cycle))
    }

    /// Renders `track` from `start` to `end`, with its tail when asked.
    fn render_bounce(
        &self,
        id: &str,
        track: TrackId,
        start: Ticks,
        end: Ticks,
        tail: bool,
    ) -> Result<(Vec<f32>, u32), ShellError> {
        let doc = self.editor.document();
        let prepared = prepared_track(doc, track).map_err(|why| failed(id, why))?;
        render_doc(self, id, &prepared, start, end - start, tail)
    }

    /// Writes one bounce file through the processed-file writer (R19).
    fn write_bounce(
        &mut self,
        id: &str,
        name: &str,
        frames: Vec<f32>,
        rate: u32,
        channels: Channels,
        files: &mut Vec<String>,
    ) -> Result<ContentHash, ShellError> {
        let (frames, count) = into_channels(frames, channels);
        let total = frames.len() as u64 / u64::from(count);
        let request = Request {
            kind: Kind::Bounce,
            name: name.to_owned(),
            spec: bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: rate,
                channels: count,
            },
            frames: total,
            produce: Box::new(move |sink| sink.write(&frames)),
        };
        let landed: Rc<RefCell<Option<Written>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&landed);
        self.start_processed(request, move |_shell, written| {
            *slot.borrow_mut() = Some(written);
            Ok(())
        })
        .map_err(|error| failed(id, error))?;
        self.wait_processed();
        let written = landed
            .borrow_mut()
            .take()
            .ok_or_else(|| failed(id, "the bounce file couldn't be written"))?;
        files.push(written.path.to_string_lossy().into_owned());
        Ok(ContentHash(*written.hash.as_bytes()))
    }

    /// `track.bounce_to_new_track` (R22, R24).
    fn bounce_to_new_track(&mut self, args: BounceToNewTrackArgs) -> Result<Value, ShellError> {
        let id = ids::BOUNCE_TO_NEW_TRACK;
        let target = self.bounce_target(&args)?;
        let doc = self.editor.document().clone();
        let (by_track, start, end) = editing::material(&doc, &target);
        let mut files = Vec::new();
        let mut made = Vec::new();
        // A group bounces as one track, after its mixer (R24).
        let groups = target_groups(&doc, &target);
        for group in &groups {
            let (from, to) = group_span(&doc, *group, &target, start, end);
            let bounced = self.bounce_group(id, *group, args.channels, from, to, &mut files)?;
            made.push((*group, self.editor.new_id(), bounced));
        }
        for (source, clips) in by_track {
            if doc
                .track(source)
                .is_some_and(|track| track.kind() == TrackKind::Group)
            {
                continue;
            }
            // A track inside a group the target names is in the group's
            // bounce, so it gets no track of its own (R24).
            if groups.iter().any(|group| group_holds(&doc, *group, source)) {
                continue;
            }
            // Spec: freeze-and-bounce R30. A source whose material is all
            // deactivated (or on take lanes, which the target leaves out)
            // gets no track.
            if clips.iter().all(|clip| clip.mute) {
                continue;
            }
            let mut clips = clips;
            clips.sort_by_key(|clip| (clip.start, clip.id));
            let bounced =
                self.bounce_material(id, source, &clips, start, end, args.channels, &mut files)?;
            made.push((source, self.editor.new_id(), bounced));
        }
        if made.is_empty() {
            return Err(unavailable(id, "nothing is selected"));
        }
        let clips: Vec<ClipId> = made
            .iter()
            .flat_map(|(_, _, bounced)| bounced.clip_ids())
            .collect();
        let new_tracks: Vec<TrackId> = made.iter().map(|(_, track, _)| *track).collect();
        let look = &self.prefs.look;
        self.editor.transact("Bounce to new tracks", |editor| {
            for (source, track, bounced) in &made {
                editor
                    .apply(Command::BounceToNewTrack {
                        source: *source,
                        track: *track,
                        bounced: bounced.clone(),
                    })
                    .map_err(|error| failed(id, error.to_string()))?;
            }
            crate::colours::colour_new(editor, look, &new_tracks, &clips)
                .map_err(|error| failed(id, error.to_string()))?;
            // R22: the source material is switched off.
            switch_off(editor, &target).map_err(|error| failed(id, error))?;
            Ok(())
        })?;
        self.changed();
        Ok(to_value(&BounceResult {
            tracks: made.iter().map(|(_, track, _)| *track).collect(),
            clips,
            files,
        }))
    }

    /// What a bounce to a new track acts on: the clips it names, on the
    /// tracks it names, or a span of time on the tracks (R22).
    fn bounce_target(&self, args: &BounceToNewTrackArgs) -> Result<Target, ShellError> {
        let id = ids::BOUNCE_TO_NEW_TRACK;
        match (args.clips.clone(), args.tracks.clone()) {
            (Some(clips), tracks) => {
                let doc = self.editor.document();
                let clips: Vec<ClipId> = clips
                    .into_iter()
                    .filter(|clip| {
                        // Spec: freeze-and-bounce R30. A take lane's clip is
                        // no material to bounce.
                        doc.clip(*clip).is_some()
                            && doc.clip_lane(*clip).is_none()
                            && tracks.as_ref().is_none_or(|tracks| {
                                doc.clip_track(*clip)
                                    .is_some_and(|track| tracks.contains(&track))
                            })
                    })
                    .collect();
                if clips.is_empty() {
                    return Err(unavailable(id, "no clip is selected"));
                }
                Ok(Target::Clips(clips))
            }
            (None, tracks) => editing::target(
                self.editor.document(),
                &self.arrangement,
                TargetArgs {
                    clips: None,
                    tracks,
                    lanes: None,
                    start: args.start.clone(),
                    end: args.end.clone(),
                    linked: Some(false),
                },
            )
            .map_err(|failure| edit_error(id, failure)),
        }
    }

    /// Renders the material of a source track and returns what the new
    /// track holds (R18, R22).
    #[expect(
        clippy::too_many_arguments,
        reason = "the command, the track, the material, its span, the channels, and the files"
    )]
    fn bounce_material(
        &mut self,
        id: &str,
        source: TrackId,
        clips: &[Clip],
        start: Ticks,
        end: Ticks,
        channels: Channels,
        files: &mut Vec<String>,
    ) -> Result<BouncedTrack, ShellError> {
        let doc = self.editor.document().clone();
        let held = doc
            .track(source)
            .ok_or_else(|| failed(id, "no such track"))?;
        if held.kind() == TrackKind::Group {
            // R24: a group's bounce is taken after its mixer.
            return self.bounce_group(id, source, channels, start, end, files);
        }
        if held.kind() != TrackKind::Audio && held.kind() != TrackKind::Midi {
            return Err(unavailable(id, "a return track can't be bounced"));
        }
        // R22: from the Session View the clips go in the same scenes' slots.
        let scenes: Vec<Option<SceneId>> =
            clips.iter().map(|clip| clip_scene(&doc, clip.id)).collect();
        if scenes.iter().any(Option::is_some) && !scenes.iter().all(Option::is_some) {
            return Err(unavailable(
                id,
                "the selection holds Arrangement and Session clips: bounce one or the other",
            ));
        }
        if !scenes.is_empty() && scenes.iter().all(Option::is_some) {
            let mut slots = Vec::new();
            for (clip, scene) in clips.iter().zip(scenes) {
                let scene = scene.expect("a scene");
                let (media, start_sec, end_sec, cycle) =
                    self.bounce_session_clip(id, source, scene, clip, channels, files)?;
                let region = FrozenClip {
                    clip: clip.id,
                    media,
                    start: start_sec,
                    end: end_sec,
                    tempo: doc.settings().tempo.bpm_at(Ticks::ZERO).get(),
                    length: clip.length,
                    original: ClipData::Midi(MidiData::default()),
                    scene: Some(scene),
                    cycle,
                };
                slots.push((
                    scene,
                    Clip {
                        id: self.editor.new_id(),
                        name: bounced_name(&clip.name),
                        start: Ticks::ZERO,
                        length: clip.length,
                        offset: Ticks::ZERO,
                        mute: false,
                        data: FrozenTrack::default().audio_for(&region),
                        envelopes: Vec::new(),
                        launch: clip.launch,
                        // Spec: scales R7. A bounced clip keeps its source's
                        // scale.
                        scale: clip.scale,
                        // A bounce plays the groove, so the audio holds it already.
                        groove: None,
                        colour: None,
                    },
                ));
            }
            return Ok(BouncedTrack {
                name: bounced_name(held.name()),
                mixer: bounced_mixer(held),
                sends: held.sends().collect(),
                output: held.routing().output,
                automation: mixer_automation(held),
                clips: Vec::new(),
                slots,
            });
        }
        let lane = self.bounce_clips(id, source, clips, start, end, channels, false, files)?;
        Ok(BouncedTrack {
            name: bounced_name(held.name()),
            mixer: bounced_mixer(held),
            sends: held.sends().collect(),
            output: held.routing().output,
            automation: mixer_automation(held),
            clips: lane,
            slots: Vec::new(),
        })
    }

    /// `track.paste_bounced_audio` (R25).
    fn paste_bounced_audio(&mut self, args: PasteBouncedAudioArgs) -> Result<Value, ShellError> {
        let id = ids::PASTE_BOUNCED_AUDIO;
        let Some(copied) = self
            .clipboard
            .as_ref()
            .and_then(Clipboard::material)
            .filter(|copied| !copied.lanes.is_empty())
        else {
            return Err(unavailable(id, "there's nothing on the clipboard to paste"));
        };
        // R25: with the copied source deleted, the command is unavailable.
        let sources: Vec<TrackId> = copied.source.clone();
        let copied_start = copied.start;
        let copied_length = copied.length;
        for source in &sources {
            if self.editor.document().track(*source).is_none() {
                return Err(unavailable(id, "the copied source is gone"));
            }
        }
        let destination = self.paste_destination(&args)?;
        // One destination per source track: a track's take lanes share its
        // distance, and the lanes run top to bottom.
        let mut distances: Vec<usize> = copied.lanes.iter().map(|lane| lane.distance).collect();
        distances.dedup();
        let tracks = self.destination_tracks(&destination, &distances)?;
        let mut files = Vec::new();
        let mut rendered = Vec::new();
        for source in sources.iter() {
            let (media, seconds) = self.render_source(
                id,
                *source,
                copied_start,
                copied_length,
                args.channels,
                &mut files,
            )?;
            rendered.push((media, seconds));
        }
        let at = self.paste_at(&args)?;
        let length = copied_length;
        let tempo = self.editor.document().settings().tempo.bpm_at(at).get();
        // The audio each source gives: a clip playing the rendered file.
        let mut clips = Vec::new();
        for (index, (media, seconds)) in rendered.iter().enumerate() {
            let clip_id = self.editor.new_id();
            let region = FrozenClip {
                clip: clip_id,
                media: *media,
                start: 0.0,
                end: *seconds,
                tempo,
                length,
                original: ClipData::Midi(MidiData::default()),
                scene: None,
                cycle: None,
            };
            clips.push((
                tracks[index],
                Clip {
                    id: clip_id,
                    name: String::new(),
                    start: at,
                    length,
                    offset: Ticks::ZERO,
                    mute: false,
                    data: FrozenTrack::default().audio_for(&region),
                    envelopes: Vec::new(),
                    launch: Default::default(),
                    // Spec: scales R7. A clip made from nothing takes the
                    // set's current scale.
                    scale: self.editor.document().settings().scale,
                    // A bounce plays the groove, so the audio holds it already.
                    groove: None,
                    colour: None,
                },
            ));
        }
        // An empty MIDI track becomes an audio track first (R25).
        let conversions = self.midi_conversions(&tracks)?;
        let scene = match destination {
            Destination::Track(_) | Destination::Lane(..) => None,
            Destination::Slot(_, scene) => Some(scene),
        };
        // The place of the take lane among its track's, which each source's
        // track puts its audio at.
        let lane_place = match destination {
            Destination::Lane(track, lane) => self
                .editor
                .document()
                .track(track)
                .and_then(|held| held.take_lanes().position(|held| held.id == lane)),
            _ => None,
        };
        let look = &self.prefs.look;
        let made = self.editor.transact("Paste bounced audio", |editor| {
            for (track, bounced) in &conversions {
                editor
                    .apply(Command::BounceTrackInPlace {
                        track: *track,
                        bounced: bounced.clone(),
                    })
                    .map_err(|error| failed(id, error.to_string()))?;
            }
            let mut created = Vec::new();
            let mut placements: Vec<(TrackId, Vec<Clip>)> = Vec::new();
            for (track, clip) in &clips {
                match scene {
                    None => {
                        let mut clip = clip.clone();
                        clip.start = at;
                        created.push(clip.id);
                        placements.push((*track, vec![clip]));
                    }
                    Some(scene) => {
                        let mut clip = clip.clone();
                        clip.start = Ticks::ZERO;
                        if let Some(existing) = editor.document().slot(*track, scene) {
                            let existing = existing.id;
                            editor
                                .apply(Command::DeleteClip { clip: existing })
                                .map_err(|error| failed(id, error.to_string()))?;
                        }
                        created.push(clip.id);
                        editor
                            .apply(Command::CreateSlotClip {
                                track: *track,
                                scene,
                                clip,
                            })
                            .map_err(|error| failed(id, error.to_string()))?;
                    }
                }
            }
            if let Some(place) = lane_place {
                created.clear();
                for (track, clips) in placements {
                    // Spec: freeze-and-bounce R25. A lane is added where the
                    // track has none at that place.
                    while editor
                        .document()
                        .track(track)
                        .is_some_and(|held| held.take_lanes().len() <= place)
                    {
                        let name = editor
                            .document()
                            .track(track)
                            .map(Track::next_take_name)
                            .unwrap_or_default();
                        let lane: TakeLaneId = editor.new_id();
                        editor
                            .apply(Command::AddTakeLane {
                                track,
                                lane: NewTakeLane {
                                    id: Some(lane),
                                    name,
                                    clips: Vec::new(),
                                },
                                index: None,
                            })
                            .map_err(|error| failed(id, error.to_string()))?;
                    }
                    let lane = editor
                        .document()
                        .track(track)
                        .and_then(|held| held.take_lanes().nth(place).map(|lane| lane.id))
                        .expect("the lane was just added");
                    created.extend(
                        editing::place_on_lane(editor, track, lane, clips)
                            .map_err(|failure| edit_error(id, failure))?,
                    );
                }
            } else if !placements.is_empty() {
                let ids = editing::place_on_main(editor, placements)
                    .map_err(|failure| edit_error(id, failure))?;
                created = ids;
            }
            crate::colours::colour_new(editor, look, &[], &created)
                .map_err(|error| failed(id, error.to_string()))?;
            Ok(created)
        });
        let made = made?;
        self.changed();
        Ok(to_value(&BounceResult {
            tracks: Vec::new(),
            clips: made,
            files,
        }))
    }

    /// The tracks the pasted audio goes on: the destination track and the
    /// tracks after it, one for each source (R25).
    fn destination_tracks(
        &self,
        destination: &Destination,
        distances: &[usize],
    ) -> Result<Vec<TrackId>, ShellError> {
        let id = ids::PASTE_BOUNCED_AUDIO;
        let doc = self.editor.document();
        let lanes = editing::lanes(doc);
        let base_track = match destination {
            Destination::Track(track)
            | Destination::Slot(track, _)
            | Destination::Lane(track, _) => *track,
        };
        let base = lanes
            .iter()
            .position(|id| *id == base_track)
            .ok_or_else(|| unavailable(id, "the destination isn't a track in the Arrangement"))?;
        let mut tracks = Vec::with_capacity(distances.len());
        for distance in distances {
            let track = lanes
                .get(base + distance)
                .copied()
                .ok_or_else(|| unavailable(id, "there aren't enough tracks below to paste onto"))?;
            tracks.push(track);
        }
        Ok(tracks)
    }

    /// The MIDI tracks that become audio tracks before the audio is
    /// pasted on them (R25): the empty ones.
    fn midi_conversions(
        &self,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, BouncedTrack)>, ShellError> {
        let id = ids::PASTE_BOUNCED_AUDIO;
        let doc = self.editor.document();
        let mut conversions = Vec::new();
        for track in tracks {
            let Some(held) = doc.track(*track) else {
                return Err(unavailable(id, "no such track"));
            };
            match held.kind() {
                TrackKind::Audio => {}
                TrackKind::Midi => {
                    if held.clips().next().is_some()
                        || held.slots().next().is_some()
                        || held.take_lanes().next().is_some()
                        || held.devices().next().is_some()
                    {
                        return Err(unavailable(
                            id,
                            "the MIDI track isn't empty: paste onto an empty one",
                        ));
                    }
                    conversions.push((
                        *track,
                        BouncedTrack {
                            name: held.name().to_owned(),
                            mixer: *held.mixer(),
                            sends: held.sends().collect(),
                            output: held.routing().output,
                            automation: mixer_automation(held),
                            clips: Vec::new(),
                            slots: Vec::new(),
                        },
                    ));
                }
                _ => {
                    return Err(unavailable(
                        id,
                        "bounced audio goes on an audio track, an empty MIDI track, or a take lane",
                    ));
                }
            }
        }
        Ok(conversions)
    }

    /// Where a paste of bounced audio goes: an Arrangement track, or a
    /// Session slot (R25).
    fn paste_destination(&self, args: &PasteBouncedAudioArgs) -> Result<Destination, ShellError> {
        let id = ids::PASTE_BOUNCED_AUDIO;
        if let Some(lane) = args.lane {
            let doc = self.editor.document();
            let track = doc
                .take_lane_track(lane)
                .ok_or_else(|| unavailable(id, "no such take lane"))?;
            if doc.track(track).map(Track::kind) != Some(TrackKind::Audio) {
                return Err(unavailable(
                    id,
                    "audio can only go on an audio track's take lane",
                ));
            }
            return Ok(Destination::Lane(track, lane));
        }
        if let Some(scene) = args.scene {
            let track = args.track.or_else(|| {
                self.session
                    .first_slot()
                    .map(|slot| slot.track)
                    .or_else(|| self.arrangement.selected_tracks().first().copied())
            });
            let track =
                track.ok_or_else(|| unavailable(id, "no track is selected to paste onto"))?;
            return Ok(Destination::Slot(track, scene));
        }
        if let Some(slot) = self.session.first_slot()
            && self.main_view == MainView::Session
        {
            return Ok(Destination::Slot(slot.track, slot.scene));
        }
        let track = args
            .track
            .or_else(|| self.arrangement.selected_tracks().first().copied())
            .ok_or_else(|| unavailable(id, "no track is selected to paste onto"))?;
        Ok(Destination::Track(track))
    }

    /// Where the pasted audio starts in the Arrangement (R25).
    fn paste_at(&self, args: &PasteBouncedAudioArgs) -> Result<Ticks, ShellError> {
        let id = ids::PASTE_BOUNCED_AUDIO;
        match &args.at {
            Some(at) => at
                .resolve(&self.editor.document().settings().time_signatures)
                .map_err(|message| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                }),
            None => Ok(self
                .arrangement
                .time_selection()
                .map_or(self.arrangement.insert_marker(), |(start, _)| start)),
        }
    }

    /// Renders a source track as it is now, over the copied material's
    /// span (R25).
    fn render_source(
        &mut self,
        id: &str,
        source: TrackId,
        start: Ticks,
        length: Ticks,
        channels: Channels,
        files: &mut Vec<String>,
    ) -> Result<(ContentHash, f64), ShellError> {
        let doc = self.editor.document().clone();
        let (frames, rate) = self.render_bounce(id, source, start, start + length, false)?;
        let seconds = frames.len() as f64 / f64::from(RENDER_CHANNELS) / f64::from(rate);
        let label = crate::routing::track_label(&doc, source);
        let media = self.write_bounce(
            id,
            &format!("{label} Bounce [{}].wav", crate::recording::stamp()),
            frames,
            rate,
            channels,
            files,
        )?;
        Ok((media, seconds))
    }
}

/// The clips a bounce made: the Arrangement lane's, and the Session slots'.
type Bounced = (Vec<Clip>, Vec<(SceneId, Clip)>);

/// Where a paste of bounced audio goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    /// A track in the Arrangement, and the distance from it.
    Track(TrackId),
    /// A Session slot.
    Slot(TrackId, SceneId),
    /// A take lane (R25, R30).
    Lane(TrackId, TakeLaneId),
}

/// The scene a clip is in, if it sits in a Session slot.
fn clip_scene(doc: &Document, clip: ClipId) -> Option<SceneId> {
    let track = doc.clip_track(clip)?;
    doc.track(track)?
        .slots()
        .find(|(_, held)| held.id == clip)
        .map(|(scene, _)| scene)
}

/// The name a bounced track or clip takes: its source's name with
/// " (Bounce)" after it, or "(Bounce)" for an unnamed one (R21).
fn bounced_name(name: &str) -> String {
    if name.is_empty() {
        "(Bounce)".to_owned()
    } else {
        format!("{name} (Bounce)")
    }
}

/// The mixer a bounced track takes: its source's, with the arm switch off
/// (R21).
fn bounced_mixer(track: &Track) -> bl_model::Mixer {
    let mut mixer = *track.mixer();
    mixer.arm = false;
    mixer
}

/// The automation a bounced track takes: its source's mixer and send
/// envelopes, since the devices' automation is in the audio (R21).
fn mixer_automation(track: &Track) -> Vec<BouncedAutomation> {
    track
        .automation()
        .filter(|(target, _)| {
            matches!(
                target,
                AutomationTarget::Mixer { .. } | AutomationTarget::Send { .. }
            )
        })
        .map(|(target, automation)| BouncedAutomation {
            target: *target,
            envelope: automation.envelope.clone(),
            enabled: automation.enabled,
        })
        .collect()
}

/// The file's frames and channels for the `channels` argument (R20): one
/// channel when the render's two are equal at every frame and two
/// otherwise, the mean of the two for `mono`, and two for `stereo`.
fn into_channels(frames: Vec<f32>, channels: Channels) -> (Vec<f32>, u16) {
    match channels {
        Channels::Auto => {
            let equal = frames
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| frame[0].to_bits() == frame[1].to_bits());
            if equal {
                (
                    frames
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|frame| frame[0])
                        .collect(),
                    1,
                )
            } else {
                (frames, 2)
            }
        }
        Channels::Mono => (
            frames
                .as_chunks::<2>()
                .0
                .iter()
                .map(|frame| (frame[0] + frame[1]) * 0.5)
                .collect(),
            1,
        ),
        Channels::Stereo => (frames, 2),
    }
}

/// A copy of `doc` that renders a group's output after its mixer, from
/// what reaches Main (R23): the group and its tracks, the return tracks
/// its tracks feed to wherever their output reaches Main, and nothing
/// else, with Main's devices and fader out.
fn prepared_group(doc: &Document, group: TrackId) -> Result<Document, String> {
    let mut doc = crate::export::prepare_document_for_track(doc, group, false)?;
    // The return tracks its tracks feed are audible, so their output
    // reaches Main (R23). Solo in Place is on again, so solo doesn't
    // silence them.
    doc.apply(&Command::SetSoloInPlace { enabled: true })
        .map_err(|error| error.to_string())?;
    let returns: Vec<TrackId> = doc.returns().collect();
    for ret in returns {
        if reaches_main(&doc, ret) && group_feeds(&doc, group, ret) {
            doc.apply(&Command::SetMute {
                track: ret,
                mute: false,
            })
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(doc)
}

/// Whether a track's output reaches Main.
fn reaches_main(doc: &Document, track: TrackId) -> bool {
    doc.track(track).is_some_and(|track| {
        matches!(
            track.routing().output,
            bl_model::Output::Default | bl_model::Output::Main
        )
    })
}

/// Whether a track inside the group, at any depth, sends to `ret`.
fn group_feeds(doc: &Document, group: TrackId, ret: TrackId) -> bool {
    let mut inside = vec![group];
    let mut found = false;
    while let Some(id) = inside.pop() {
        let Some(track) = doc.track(id) else {
            continue;
        };
        for (to, level) in track.sends() {
            if to == ret && level > 0.0 {
                found = true;
            }
        }
        inside.extend(track.children());
    }
    found
}

/// Switches a target's material off (R22): the selected clips' activators
/// go off, and with a time selection the clips its edges cross are split
/// there and the parts inside switched off.
fn switch_off(editor: &mut Editor, target: &Target) -> Result<(), String> {
    match target {
        Target::Clips(clips) => {
            for clip in clips {
                editor
                    .apply(Command::SetClipMute {
                        clip: *clip,
                        mute: true,
                    })
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        Target::Time { places, start, end } => {
            let tracks = &editing::main_tracks(places);
            for track in tracks {
                // A group's own tracks are switched off with it (R24).
                let mut lanes = vec![*track];
                let mut pending = vec![*track];
                while let Some(id) = pending.pop() {
                    if let Some(track) = editor.document().track(id) {
                        for child in track.children() {
                            lanes.push(child);
                            pending.push(child);
                        }
                    }
                }
                for track in lanes {
                    let crossing: Vec<ClipId> = editor
                        .document()
                        .track(track)
                        .map(|track| {
                            track
                                .clips()
                                .filter(|clip| clip.start < *end && clip.end() > *start)
                                .map(|clip| clip.id)
                                .collect()
                        })
                        .unwrap_or_default();
                    for clip in crossing {
                        let Some(held) = editor.document().clip(clip).cloned() else {
                            continue;
                        };
                        let mut inside = clip;
                        if held.start < *start && *start < held.end() {
                            inside = placing::split_off(editor, clip, *start)
                                .map_err(|error| error.to_string())?;
                        }
                        let held_end = editor
                            .document()
                            .clip(inside)
                            .map_or(held.end(), |clip| clip.end());
                        if *start < *end && *end < held_end {
                            placing::split_off(editor, inside, *end)
                                .map_err(|error| error.to_string())?;
                        }
                        editor
                            .apply(Command::SetClipMute {
                                clip: inside,
                                mute: true,
                            })
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            Ok(())
        }
    }
}

/// The clips' first start and last end.
fn arrangement_bounds(clips: &[Clip]) -> (Ticks, Ticks) {
    let first = clips.first().expect("a clip");
    let mut start = first.start;
    let mut end = first.end();
    for clip in clips {
        start = start.min(clip.start);
        end = end.max(clip.end());
    }
    (start, end)
}

/// Where each clip's bounced audio sits in the file, and how long the
/// bounced clip is: the clip's own span, grown to hold its tail, up to the
/// next clip's start (R18).
fn clip_spans(
    doc: &Document,
    clips: &[Clip],
    start: Ticks,
    frames: &[f32],
    rate: u32,
    total: f64,
) -> Vec<(f64, f64, Ticks)> {
    clips
        .iter()
        .enumerate()
        .map(|(index, clip)| {
            let from = seconds_between(doc, start, clip.start);
            let to = seconds_between(doc, start, clip.end());
            let next = clips.get(index + 1).map_or(total, |next| {
                seconds_between(doc, start, next.start).min(total)
            });
            let tail = quiet_end(frames, rate, to, next);
            let length = clip.length + frames_to_ticks(doc, rate, (tail - to).max(0.0));
            (from, tail.max(to), length)
        })
        .collect()
}

/// The groups a target names: the tracks in it that are groups, and the
/// groups the selected clips are inside (R24).
fn target_groups(doc: &Document, target: &Target) -> Vec<TrackId> {
    let mut groups: Vec<TrackId> = Vec::new();
    let push = |track: TrackId, groups: &mut Vec<TrackId>| {
        if doc
            .track(track)
            .is_some_and(|track| track.kind() == TrackKind::Group)
            && !groups.contains(&track)
        {
            groups.push(track);
        }
    };
    match target {
        Target::Clips(clips) => {
            for clip in clips {
                if let Some(track) = doc.clip_track(*clip)
                    && let Some(parent) = doc.parent(track)
                {
                    push(parent, &mut groups);
                }
            }
        }
        Target::Time { places, .. } => {
            for track in editing::main_tracks(places) {
                push(track, &mut groups);
            }
        }
    }
    groups
}

/// The span a group's bounce covers: the time selection's, or the span of
/// the selected clips inside the group (R24).
fn group_span(
    doc: &Document,
    group: TrackId,
    target: &Target,
    start: Ticks,
    end: Ticks,
) -> (Ticks, Ticks) {
    if let Target::Time { .. } = target {
        return (start, end);
    }
    let inside: Vec<&Clip> = match target {
        Target::Clips(clips) => clips
            .iter()
            .filter_map(|clip| doc.clip(*clip))
            .filter(|clip| {
                doc.clip_track(clip.id)
                    .is_some_and(|track| group_holds(doc, group, track))
            })
            .collect(),
        Target::Time { .. } => Vec::new(),
    };
    match (inside.first(), inside.last()) {
        (Some(first), Some(last)) => (first.start, last.end()),
        _ => (start, end),
    }
}

/// Whether a track is inside a group, at any depth.
fn group_holds(doc: &Document, group: TrackId, track: TrackId) -> bool {
    let mut current = Some(track);
    while let Some(id) = current {
        if id == group {
            return true;
        }
        current = doc.parent(id);
    }
    false
}

/// The end of the last clip in the set, for a group's render (R23).
fn arrangement_end(doc: &Document) -> Ticks {
    let mut end = Ticks::ZERO;
    for track in doc.tracks() {
        for clip in track.clips() {
            end = end.max(clip.end());
        }
    }
    end
}

/// Where a tail falls silent: the last frame above [`FLOOR`] between
/// `from` and `to` seconds, in seconds; `to` or `from` when there is
/// none.
fn quiet_end(frames: &[f32], rate: u32, from: f64, to: f64) -> f64 {
    let channels = usize::from(RENDER_CHANNELS);
    let first = (from * f64::from(rate)).floor().max(0.0) as usize * channels;
    let last = ((to * f64::from(rate)).ceil() as usize * channels).min(frames.len());
    let mut end = from;
    for index in (first..last).step_by(channels) {
        if frames.get(index).is_some_and(|sample| sample.abs() > FLOOR) {
            end = index as f64 / f64::from(rate) / f64::from(RENDER_CHANNELS);
        }
    }
    end.min(to)
}

fn unavailable(id: &str, reason: impl ToString) -> ShellError {
    ShellError::Unavailable {
        command: id.to_owned(),
        reason: reason.to_string(),
    }
}

fn failed(id: &str, message: impl ToString) -> ShellError {
    ShellError::Failed {
        command: id.to_owned(),
        message: message.to_string(),
    }
}

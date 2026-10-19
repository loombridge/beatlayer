//! Crop, Consolidate, and Consolidate Time to New Scene through the shell
//! (WP-4.13): the new files go through WP-4.20's processed-file writer, and
//! each edit lands as one undo step when its file is whole. The behaviour is
//! in the audio clip editing spec, R14 to R28.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, MidiData, SceneId, TrackId,
    TrackKind, WarpMarker, consolidated_midi,
};
use bl_shell_spec::audio_editing::{
    self as ids, ConsolidateResult, ConsolidateSceneArgs, ConsolidateSceneResult, CropArgs,
    CropResult,
};
use bl_shell_spec::editing::TargetArgs;
use serde_json::Value;

use super::*;
use crate::arrangement::editing;
use crate::arrangement::placing::clear;
use crate::processed::{Kind, Request, Written};

/// The longest a consolidate render may run: a day, as an export's is.
const LONGEST: f64 = 86_400.0;

/// How close two positions may be and still name the same point: a
/// millionth of a frame at 192 kHz.
const SLACK: f64 = 1e-9;

/// One track's consolidated clip: its place in the material the range
/// covered, the track, the new clip, and the file it plays.
type Landed = (usize, TrackId, ClipId, String);

impl Shell {
    /// Runs one of the audio clip editing commands (WP-4.13).
    pub(super) fn run_audio_editing(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::CROP => self.crop(id, args),
            ids::CONSOLIDATE => self.consolidate(id, args),
            ids::CONSOLIDATE_TO_SCENE => self.consolidate_to_scene(id, args),
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether one of the audio clip editing commands can run with `args`:
    /// the menu items' enabled states read this (R28).
    pub(super) fn can_audio_edit(&self, id: &str, args: &Value) -> bool {
        match id {
            ids::CROP => self.crop_target(args).is_ok(),
            ids::CONSOLIDATE | ids::CONSOLIDATE_TO_SCENE => {
                match Self::args::<TargetArgs>(id, args.clone()) {
                    Ok(args) => self.material_of(id, &args).is_ok(),
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }

    /// `clip.crop` (R14 to R18): an audio clip's region is written as a new
    /// file and it plays that; a MIDI clip loses the notes outside its region.
    fn crop(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: CropArgs = Self::args(id, args)?;
        let span_of_args = (args.start.clone(), args.end.clone());
        let clip = self.crop_target_value(id, &args)?;
        let span = match span_of_args {
            (Some(start), Some(end)) => {
                let signatures = &self.editor.document().settings().time_signatures;
                let start = start.resolve(signatures).map_err(|why| invalid(id, why))?;
                let end = end.resolve(signatures).map_err(|why| invalid(id, why))?;
                if end <= start {
                    return Err(invalid(id, "the span's `end` must be after its `start`"));
                }
                Some((start, end))
            }
            (None, None) => None,
            _ => return Err(invalid(id, "give `start` and `end` together")),
        };
        let value = self.clip_of(id, clip)?;
        if let Some((start, end)) = span {
            self.check_span(id, &value, start, end)?;
        }
        let removed = crop_removes(&value, span);
        match &value.data {
            ClipData::Midi(_) => self.crop_midi(id, clip, &value, span, removed),
            ClipData::Audio(_) => self.crop_audio(id, clip, &value, span, removed),
        }
    }

    /// Crops a MIDI clip (R17): the notes that start outside its region go,
    /// and its markers and loop begin at the region's start. No file is
    /// written, so the crop is one document command.
    fn crop_midi(
        &mut self,
        id: &str,
        clip: ClipId,
        value: &Clip,
        span: Option<(Ticks, Ticks)>,
        removed: bool,
    ) -> Result<Value, ShellError> {
        let ClipData::Midi(midi) = &value.data else {
            return Err(failed(id, "the clip isn't a MIDI clip"));
        };
        let command = match span {
            Some((start, end)) => Command::CropClipToRange {
                clip,
                media: None,
                start,
                end,
            },
            None => {
                let (from, to) = midi_region(value, midi);
                Command::CropClip {
                    clip,
                    media: None,
                    from: from.get() as f64,
                    to: to.get() as f64,
                }
            }
        };
        self.apply_one(id, "Crop clip", command)?;
        Ok(to_value(&CropResult {
            clip,
            media: None,
            path: None,
            removed,
        }))
    }

    /// Crops an audio clip (R14, R15, R16): its region's frames are written
    /// through the processed-file writer and the clip plays the new file.
    fn crop_audio(
        &mut self,
        id: &str,
        clip: ClipId,
        value: &Clip,
        span: Option<(Ticks, Ticks)>,
        removed: bool,
    ) -> Result<Value, ShellError> {
        let ClipData::Audio(audio) = &value.data else {
            return Err(failed(id, "the clip isn't an audio clip"));
        };
        let audio = audio.clone();
        let decoded = self
            .edited_audio(audio.media, &[])
            .ok_or_else(|| failed(id, "the clip's file is missing"))?;
        let rate = decoded.sample_rate();
        let frames = decoded.frames();
        let file_seconds = frames as f64 / f64::from(rate);
        // The region in the form the clip shows: seconds in an unwarped
        // clip's file, ticks of a warped one's contents.
        let (from, to) = match span {
            Some((start, end)) => {
                // A warped clip's contents advance a tick per tick of the
                // song; an unwarped clip's by the time between the ticks.
                let tempo = &self.editor.document().settings().tempo;
                let at = |ticks: Ticks| tempo.ticks_to_seconds(ticks);
                if audio.warp {
                    (
                        audio.start + (start - value.start).get() as f64,
                        audio.start + (end - value.start).get() as f64,
                    )
                } else {
                    (
                        audio.start + at(start) - at(value.start),
                        audio.start + at(end) - at(value.start),
                    )
                }
            }
            None => audio.crop_region(file_seconds),
        };
        let first = ((audio.in_file_seconds(from) * f64::from(rate))
            .round()
            .max(0.0) as u64)
            .min(frames as u64);
        let last = ((audio.in_file_seconds(to) * f64::from(rate))
            .round()
            .max(0.0) as u64)
            .min(frames as u64);
        if last <= first {
            return Err(unavailable(id, "the region holds no audio"));
        }
        // Spec: R15. An unwarped clip's region is rounded to whole frames, so
        // it plays the new file's frames exactly where the old file's were; a
        // warped clip's is whole ticks, which its map plays at the nearest
        // frame.
        let (from, to) = if audio.warp {
            (from, to)
        } else {
            (
                first as f64 / f64::from(rate),
                last as f64 / f64::from(rate),
            )
        };
        let channels = decoded.channel_count();
        let stem = self
            .media
            .entry(bl_media::ContentHash::from_bytes(audio.media.0))
            .and_then(|entry| {
                entry
                    .path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Sample".to_owned());
        let request = Request {
            kind: Kind::Crop,
            name: format!("{stem} Crop.wav"),
            spec: bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: rate,
                channels: channels as u16,
            },
            frames: last - first,
            produce: Box::new(move |sink| {
                // Spec: R2. The new file holds the source's own samples, cut
                // to the region.
                let mut buffer = Vec::with_capacity(4096 * channels);
                for chunk in (first..last).step_by(4096) {
                    buffer.clear();
                    for frame in chunk..(chunk + 4096).min(last) {
                        for channel in 0..channels {
                            buffer
                                .push(decoded.channel(channel).map_or(0.0, |c| c[frame as usize]));
                        }
                    }
                    sink.write(&buffer)?;
                }
                Ok(())
            }),
        };
        let landed: Rc<RefCell<Option<Written>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&landed);
        self.start_processed(request, move |shell, written| {
            let media = ContentHash(*written.hash.as_bytes());
            let command = match span {
                Some((start, end)) => Command::CropClipToRange {
                    clip,
                    media: Some(media),
                    start,
                    end,
                },
                None => Command::CropClip {
                    clip,
                    media: Some(media),
                    from,
                    to,
                },
            };
            shell
                .editor
                .transact("Crop clip", |editor| editor.apply(command).map(drop))
                .map_err(|error| error.to_string())?;
            shell.changed();
            *slot.borrow_mut() = Some(written);
            Ok(())
        })
        .map_err(|error| failed(id, error))?;
        self.wait_processed();
        let written = landed
            .borrow_mut()
            .take()
            .ok_or_else(|| failed(id, "the new file couldn't be written"))?;
        Ok(to_value(&CropResult {
            clip,
            media: Some(ContentHash(*written.hash.as_bytes())),
            path: Some(written.path.to_string_lossy().into_owned()),
            removed,
        }))
    }

    /// `arrangement.consolidate` (R19 to R25): one new clip per track over
    /// the range, replacing the material it covers.
    fn consolidate(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: TargetArgs = Self::args(id, args)?;
        let (material, start, end) = self.material_of(id, &args)?;
        self.editor.begin("Consolidate");
        let outcome = self.consolidate_range(id, material, start, end, None);
        let (created, tracks, files) = self.finish(outcome)?;
        Ok(to_value(&ConsolidateResult {
            created,
            tracks,
            files,
        }))
    }

    /// `arrangement.consolidate_to_scene` (R26, R27): the range's material
    /// becomes a new Session scene's clips, leaving the Arrangement alone.
    fn consolidate_to_scene(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: ConsolidateSceneArgs = Self::args(id, args)?;
        let targets = TargetArgs {
            clips: None,
            tracks: args.tracks,
            start: args.start,
            end: args.end,
        };
        let (material, start, end) = self.material_of(id, &targets)?;
        let doc = self.editor.document();
        let index = match args.scene.or_else(|| self.shown_scene()) {
            Some(scene) => doc
                .scene_position(scene)
                .map(|at| at + 1)
                .ok_or_else(|| invalid(id, "no scene has that ID"))?,
            None => doc.scene_count(),
        };
        let scene: SceneId = self.editor.new_id();
        self.editor.begin("Consolidate Time to New Scene");
        let outcome = self
            .editor
            .apply(Command::CreateScene { id: scene, index })
            .map(drop)
            .map_err(|error| failed(id, error))
            .and_then(|()| self.consolidate_range(id, material, start, end, Some(scene)));
        let (created, tracks, files) = self.finish(outcome)?;
        Ok(to_value(&ConsolidateSceneResult {
            scene,
            created,
            tracks,
            files,
        }))
    }

    /// Closes the transaction a consolidate opened, keeping it if `outcome`
    /// succeeded and taking it back if it didn't.
    #[expect(
        clippy::type_complexity,
        reason = "the command's result, named where it's used"
    )]
    fn finish(
        &mut self,
        outcome: Result<Vec<(TrackId, ClipId, Option<String>)>, ShellError>,
    ) -> Result<(Vec<ClipId>, Vec<TrackId>, Vec<String>), ShellError> {
        // The clips the consolidation made, in track order.
        match outcome {
            Ok(done) => {
                if self.editor.commit().unwrap_or(false) {
                    self.dirty = true;
                }
                self.changed();
                // Spec: R19. The new clips are selected.
                let created: Vec<ClipId> = done.iter().map(|(_, clip, _)| *clip).collect();
                if !created.is_empty() {
                    self.arrangement
                        .select_clips(self.editor.document(), created.clone());
                }
                Ok((
                    created,
                    done.iter().map(|(track, ..)| *track).collect(),
                    done.iter().filter_map(|(.., path)| path.clone()).collect(),
                ))
            }
            Err(error) => {
                self.editor.cancel().ok();
                Err(error)
            }
        }
    }

    /// Consolidates `material` — each track with the clips the range covers —
    /// into one clip per track over `start`..`end`, in `scene`'s slots or,
    /// without one, in the Arrangement (R19 to R23). The caller has a
    /// transaction open, and [`Shell::finish`] closes it.
    fn consolidate_range(
        &mut self,
        id: &str,
        material: Vec<(TrackId, Vec<Clip>)>,
        start: Ticks,
        end: Ticks,
        scene: Option<SceneId>,
    ) -> Result<Vec<(TrackId, ClipId, Option<String>)>, ShellError> {
        let landed: Rc<RefCell<Vec<Landed>>> = Rc::new(RefCell::new(Vec::new()));
        let broke: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let mut order: Vec<(usize, TrackId, ClipId, Option<String>)> = Vec::new();
        for (index, entry) in material.iter().enumerate() {
            let track = entry.0;
            let clips = &entry.1;
            let kind = self.editor.document().track(track).map(|t| t.kind());
            let name = clips
                .first()
                .map(|clip| clip.name.clone())
                .unwrap_or_default();
            let new_id: ClipId = self.editor.new_id();
            match kind {
                Some(TrackKind::Midi) => {
                    let mut clip = consolidated_midi(
                        new_id,
                        name,
                        &clips.iter().collect::<Vec<_>>(),
                        start,
                        end,
                    );
                    if scene.is_some() {
                        clip.start = Ticks::ZERO;
                    }
                    let placed = self.editor.transact(label_of(scene), |editor| {
                        place(editor, track, scene, start, end, clip)
                    });
                    match placed {
                        Ok(()) => order.push((index, track, new_id, None)),
                        Err(error) => return Err(failed(id, error)),
                    }
                }
                Some(TrackKind::Audio) => {
                    let (frames, channels, gain, markers, tempo) =
                        self.consolidate_audio(id, track, start, end)?;
                    let label = crate::routing::track_label(self.editor.document(), track);
                    let file = format!("{label} Consolidate [{}].wav", crate::recording::stamp());
                    let rate = self.engine_rate();
                    let total = frames.len() as u64 / u64::from(channels);
                    let request = Request {
                        kind: Kind::Consolidate,
                        name: file,
                        spec: bl_media::EncodeSpec {
                            file_type: bl_media::FileType::Wav,
                            bit_depth: bl_media::BitDepth::Float32,
                            dither: bl_media::Dither::None,
                            sample_rate: rate,
                            channels,
                        },
                        frames: total,
                        produce: Box::new(move |sink| sink.write(&frames)),
                    };
                    let done = Rc::clone(&landed);
                    let bad = Rc::clone(&broke);
                    self.start_processed(request, move |shell, written| {
                        let audio = consolidated_audio(
                            ContentHash(*written.hash.as_bytes()),
                            gain,
                            markers,
                            tempo,
                        );
                        let clip = Clip {
                            id: new_id,
                            name,
                            start: if scene.is_some() { Ticks::ZERO } else { start },
                            length: end - start,
                            offset: Ticks::ZERO,
                            mute: false,
                            data: ClipData::Audio(audio),
                            envelopes: Vec::new(),
                            launch: bl_model::ClipLaunch::default(),
                        };
                        let placed = shell.editor.transact(label_of(scene), |editor| {
                            place(editor, track, scene, start, end, clip)
                        });
                        match placed {
                            Ok(()) => {
                                shell.changed();
                                done.borrow_mut().push((
                                    index,
                                    track,
                                    new_id,
                                    written.path.to_string_lossy().into_owned(),
                                ));
                                Ok(())
                            }
                            Err(error) => {
                                bad.set(true);
                                Err(error.to_string())
                            }
                        }
                    })
                    .map_err(|error| failed(id, error))?;
                }
                _ => {}
            }
        }
        self.wait_processed();
        if broke.get() {
            return Err(unavailable(id, "a consolidated file couldn't be written"));
        }
        let mut all = order;
        for (index, track, clip, path) in landed.borrow().iter() {
            all.push((*index, *track, *clip, Some(path.clone())));
        }
        all.sort_by_key(|(index, ..)| *index);
        Ok(all
            .into_iter()
            .map(|(_, track, clip, path)| (track, clip, path))
            .collect())
    }

    /// What an audio track's clips play over the range, rendered offline
    /// before its devices and mixer (R20), with the file's frames, the clip
    /// gain R21's normalisation lands on, the warp map that holds the beats
    /// it was rendered at, and the tempo after it (R21).
    #[expect(
        clippy::type_complexity,
        reason = "the render's outcome, named where it's used"
    )]
    fn consolidate_audio(
        &self,
        id: &str,
        track: TrackId,
        start: Ticks,
        end: Ticks,
    ) -> Result<(Vec<f32>, u16, f64, Vec<WarpMarker>, f64), ShellError> {
        let document = self.editor.document();
        let prepared = consolidate_document(document, track).map_err(|why| failed(id, why))?;
        let rate = self.engine_rate();
        let media: Arc<dyn bl_engine::MediaLibrary> = self.media.clone();
        let spec = crate::export::Render {
            start,
            length: crate::export::Length::To(end),
            sample_rate: rate,
            engine_rate: rate,
            block: crate::export::BLOCK,
        };
        let cancel = AtomicBool::new(false);
        // shortcut: the whole render is held in memory, and a very long
        // consolidation therefore takes a lot of it; upgrade to streaming
        // into the writer when a person consolidates minutes of music.
        let mut rendered: Vec<f32> = Vec::new();
        {
            let mut sink = |samples: &[f32]| {
                rendered.extend_from_slice(samples);
                Ok(())
            };
            crate::export::render(
                &prepared,
                media,
                self.plugin_hosts(),
                &spec,
                LONGEST,
                &cancel,
                &mut |_| {},
                &mut sink,
                None,
            )
            .map_err(|error| failed(id, error))?;
        }
        if rendered.is_empty() {
            return Err(unavailable(id, "the range holds no audio"));
        }
        // Spec: R20. Mono when the two channels are equal at every frame.
        let stereo = rendered.as_chunks::<2>().0;
        let mono = stereo
            .iter()
            .all(|frame| frame[0].to_bits() == frame[1].to_bits());
        let peak = rendered
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let gain = if peak > 0.0 { f64::from(peak) } else { 1.0 };
        let scale = (1.0 / gain) as f32;
        let frames: Vec<f32> = if mono {
            stereo.iter().map(|frame| frame[0] * scale).collect()
        } else {
            rendered.iter().map(|sample| sample * scale).collect()
        };
        let markers = beat_markers(document, start, end);
        let tempo = document.settings().tempo.bpm_at(end).get();
        Ok((frames, if mono { 1 } else { 2 }, gain, markers, tempo))
    }

    /// What a consolidate acts on: each track with the clips the range
    /// covers, already cut to the range, and the range itself.
    #[expect(
        clippy::type_complexity,
        reason = "each track's clips cut to the range, and the range"
    )]
    fn material_of(
        &self,
        id: &str,
        args: &TargetArgs,
    ) -> Result<(Vec<(TrackId, Vec<Clip>)>, Ticks, Ticks), ShellError> {
        let doc = self.editor.document();
        let target = editing::target(doc, &self.arrangement, args.clone())
            .map_err(|failure| edit_error(id, failure))?;
        let (by_track, start, end) = editing::material(doc, &target);
        let material: Vec<(TrackId, Vec<Clip>)> = by_track
            .into_iter()
            .filter(|(_, clips)| !clips.is_empty())
            .collect();
        if material.is_empty() {
            return Err(unavailable(id, "no clip is in the range"));
        }
        Ok((material, start, end))
    }

    /// The clip a `clip.crop` acts on: the one it names, or the selected one.
    fn crop_target(&self, args: &Value) -> Result<ClipId, ShellError> {
        let args: CropArgs = Self::args(ids::CROP, args.clone())?;
        self.crop_target_value(ids::CROP, &args)
    }

    fn crop_target_value(&self, id: &str, args: &CropArgs) -> Result<ClipId, ShellError> {
        let doc = self.editor.document();
        if let Some(clip) = args.clip {
            return doc
                .clip(clip)
                .map(|_| clip)
                .ok_or_else(|| self.no_clip(id, clip));
        }
        if let Some(clip) = self.clip_view.shown_any_clip(doc, &self.arrangement) {
            return Ok(clip);
        }
        // Spec: R16. A time selection over a track's clip crops that clip
        // (C-TRACK-020): with no clip named or shown, the first clip the
        // span covers on the selected tracks.
        if let (Some(start), Some(end)) = (&args.start, &args.end) {
            let signatures = &doc.settings().time_signatures;
            if let (Ok(start), Ok(end)) = (start.resolve(signatures), end.resolve(signatures)) {
                let selected = self.arrangement.selected_tracks();
                for track in doc.track_order() {
                    if !selected.is_empty() && !selected.contains(&track) {
                        continue;
                    }
                    if let Some(track) = doc.track(track)
                        && let Some(clip) = track
                            .clips()
                            .find(|clip| clip.start < end && clip.end() > start)
                    {
                        return Ok(clip.id);
                    }
                }
            }
        }
        Err(unavailable(id, "no clip is selected"))
    }

    /// Refuses a span that isn't one stretch of what the clip plays: one the
    /// clip goes round its loop over, or one that leaves the clip (R16).
    fn check_span(
        &self,
        id: &str,
        clip: &Clip,
        start: Ticks,
        end: Ticks,
    ) -> Result<(), ShellError> {
        if start < clip.start || end > clip.end() {
            return Err(invalid(id, "the span isn't inside the clip"));
        }
        let tempo = &self.editor.document().settings().tempo;
        // The clip's first pass, in the form the clip shows, and where the
        // span's end falls in it.
        let (asked, pass) = match &clip.data {
            ClipData::Midi(midi) => {
                let contents = midi.start + clip.length;
                let pass = if midi.looping {
                    midi.loop_end.unwrap_or(contents)
                } else {
                    midi.end.unwrap_or(contents)
                };
                (
                    (midi.start + (end - clip.start)).get() as f64,
                    pass.min(contents).get() as f64,
                )
            }
            ClipData::Audio(audio) if audio.warp => {
                let contents = audio.start + clip.length.get() as f64;
                let pass = if audio.looping {
                    audio.loop_end.unwrap_or(contents)
                } else {
                    audio.end.unwrap_or(contents)
                };
                (
                    audio.start + (end - clip.start).get() as f64,
                    pass.min(contents),
                )
            }
            ClipData::Audio(audio) => {
                // An unwarped clip's contents are seconds, and it plays at
                // most its own length of them.
                let seconds = |at: Ticks| tempo.ticks_to_seconds(at);
                let played = seconds(clip.end()) - seconds(clip.start);
                let pass = if audio.looping {
                    audio.loop_end.unwrap_or(audio.start + played)
                } else {
                    audio.end.unwrap_or(audio.start + played)
                };
                (
                    audio.start + seconds(end) - seconds(clip.start),
                    pass.min(audio.start + played),
                )
            }
        };
        if asked > pass + SLACK {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the selection goes round the clip's loop, so it isn't one stretch of \
                         its file; Consolidate writes such a stretch out"
                    .to_owned(),
            });
        }
        Ok(())
    }

    /// The scene the Session View has selected, if any.
    fn shown_scene(&self) -> Option<SceneId> {
        let state = self.session.state();
        state
            .scenes
            .first()
            .copied()
            .or_else(|| state.slots.first().map(|slot| slot.scene))
    }

    /// The clip with `clip`'s ID, or an error naming the command.
    fn clip_of(&self, id: &str, clip: ClipId) -> Result<Clip, ShellError> {
        self.editor
            .document()
            .clip(clip)
            .cloned()
            .ok_or_else(|| self.no_clip(id, clip))
    }

    fn no_clip(&self, id: &str, clip: ClipId) -> ShellError {
        invalid(id, format!("no clip has the ID {}", clip.0))
    }

    /// Runs one document command as its own undo step.
    fn apply_one(
        &mut self,
        id: &str,
        label: &'static str,
        command: Command,
    ) -> Result<(), ShellError> {
        self.editor
            .transact(label, |editor| editor.apply(command).map(drop))
            .map_err(|error| failed(id, error))?;
        self.changed();
        Ok(())
    }
}

/// The label a consolidate's edit lands under.
fn label_of(scene: Option<SceneId>) -> &'static str {
    if scene.is_some() {
        "Consolidate Time to New Scene"
    } else {
        "Consolidate"
    }
}

/// Puts a consolidated clip in its place: the new scene's slot, or the
/// Arrangement over the range the old clips covered (R19, R26).
fn place(
    editor: &mut bl_model::Editor,
    track: TrackId,
    scene: Option<SceneId>,
    start: Ticks,
    end: Ticks,
    clip: Clip,
) -> Result<(), bl_model::EditError> {
    if let Some(scene) = scene {
        return editor
            .apply(Command::CreateSlotClip { track, scene, clip })
            .map(drop);
    }
    clear(editor, track, start, end, &BTreeSet::new())?;
    editor.apply(Command::CreateClip { track, clip }).map(drop)
}

/// Whether cropping `clip` to `span`, or to the region its own markers hold,
/// would remove anything (R18): audio outside the region, or notes. The
/// shell's permission assessment is decided from this.
pub(crate) fn crop_removes(clip: &Clip, span: Option<(Ticks, Ticks)>) -> bool {
    match &clip.data {
        ClipData::Midi(midi) => {
            let (from, to) = match span {
                Some((start, end)) => (
                    midi.start + (start - clip.start),
                    midi.start + (end - clip.start),
                ),
                None => midi_region(clip, midi),
            };
            midi.notes
                .iter()
                .any(|note| note.start < from || note.start >= to)
        }
        ClipData::Audio(audio) => {
            // Spec: R14. The region the clip's own markers hold is what it
            // plays, so cropping to it removes nothing that plays; a marker
            // the clip doesn't set reaches the file's end, which only the
            // file knows, so a crop that could fall short of it removes
            // material. Cropping to a span drops everything outside it, and
            // moves the clip's edges to the span (R16).
            match span {
                Some((start, end)) => start > clip.start || end < clip.end(),
                None => {
                    audio.start > SLACK
                        || audio.end.is_some()
                        || audio.loop_start > SLACK
                        || audio.loop_end.is_some()
                }
            }
        }
    }
}

/// The region `clip.crop` keeps on a MIDI clip (R17): its loop brace while
/// its loop is on, and its Start and End markers while it's off, in ticks of
/// its contents.
pub(crate) fn midi_region(clip: &Clip, midi: &MidiData) -> (Ticks, Ticks) {
    let contents = midi.start + clip.length;
    if midi.looping {
        (
            midi.start.min(midi.loop_start),
            midi.loop_end.unwrap_or(contents).min(contents),
        )
    } else {
        (midi.start, midi.end.unwrap_or(contents).min(contents))
    }
}

/// The audio clip data a consolidate's clip plays (R21): the new file, at 0
/// dB until `gain` makes up for its normalisation, warped with the map that
/// holds the beats it was rendered at, and looped over the whole file.
fn consolidated_audio(
    media: ContentHash,
    gain: f64,
    markers: Vec<WarpMarker>,
    tempo: f64,
) -> AudioData {
    AudioData {
        media,
        start: 0.0,
        end: None,
        looping: true,
        loop_start: 0.0,
        loop_end: None,
        gain,
        fade_in: 0.0,
        fade_out: 0.0,
        warp: true,
        warp_markers: markers,
        tempo: Some(tempo),
        signature: None,
        warp_mode: bl_model::WarpMode::Beats,
        warp_settings: bl_model::WarpSettings::default(),
        ram: false,
        high_quality: true,
        transient_edits: bl_model::TransientEdits::default(),
    }
}

/// The warp markers that hold a consolidated file on the beats it was
/// rendered at (R21): the range's start and end, and each tempo segment
/// between them, at the times the song's tempo map places them.
///
/// shortcut: a tempo segment that ramps is one straight stretch of the warp
/// map between two markers, where the song's tempo map curves; upgrade to a
/// marker per beat once a rounded, ramped tempo has to null to the bit.
fn beat_markers(document: &Document, start: Ticks, end: Ticks) -> Vec<WarpMarker> {
    let tempo = &document.settings().tempo;
    let base = tempo.ticks_to_seconds(start);
    let mut beats = vec![start];
    for segment in tempo.segments() {
        let at = segment.start();
        if at > start && at < end {
            beats.push(at);
        }
    }
    beats.push(end);
    beats.sort_unstable();
    beats.dedup();
    beats
        .into_iter()
        .map(|beat| WarpMarker {
            time: (tempo.ticks_to_seconds(beat) - base).max(0.0),
            beat: beat - start,
        })
        .collect()
}

/// A document holding only `track`, for a consolidate's render (R20): its
/// clips' audio reaches Main without its devices or its mixer, and without
/// another track's.
///
/// # Errors
///
/// A message if the document refuses one of the steps.
fn consolidate_document(document: &Document, track: TrackId) -> Result<Document, String> {
    let mut doc = document.clone();
    let main = doc.main_id();
    let others: Vec<TrackId> = doc
        .track_order()
        .into_iter()
        .filter(|id| *id != track && *id != main)
        .collect();
    for other in others {
        doc.apply(&Command::DeleteTrack { track: other })
            .map_err(|error| error.to_string())?;
    }
    if doc.parent(track).is_some() {
        doc.apply(&Command::SetOutput {
            track,
            output: bl_model::Output::Main,
        })
        .map_err(|error| error.to_string())?;
    }
    let devices: Vec<bl_model::DeviceId> = doc
        .track(track)
        .map(|track| track.devices_deep().map(|device| device.id).collect())
        .unwrap_or_default();
    let main_devices: Vec<bl_model::DeviceId> =
        doc.main().devices().map(|device| device.id).collect();
    let sends: Vec<TrackId> = doc
        .track(track)
        .map(|track| track.sends().map(|(to, _)| to).collect())
        .unwrap_or_default();
    for device in devices.into_iter().chain(main_devices) {
        doc.apply(&Command::SetDeviceEnabled {
            device,
            enabled: false,
        })
        .map_err(|error| error.to_string())?;
    }
    for to in sends {
        doc.apply(&Command::SetSend {
            track,
            to,
            level: 0.0,
        })
        .map_err(|error| error.to_string())?;
    }
    doc.apply(&Command::SetMute { track, mute: false })
        .map_err(|error| error.to_string())?;
    doc.apply(&Command::SetSolo { track, solo: false })
        .map_err(|error| error.to_string())?;
    for id in [track, main] {
        doc.apply(&Command::SetVolume {
            track: id,
            volume: 1.0,
        })
        .map_err(|error| error.to_string())?;
        doc.apply(&Command::SetPan {
            track: id,
            pan: 0.0,
        })
        .map_err(|error| error.to_string())?;
    }
    Ok(doc)
}

/// An argument error naming the command.
fn invalid(id: &str, message: impl ToString) -> ShellError {
    ShellError::InvalidArgs {
        command: id.to_owned(),
        message: message.to_string(),
    }
}

/// A "not now" error naming the command.
fn unavailable(id: &str, reason: impl ToString) -> ShellError {
    ShellError::Unavailable {
        command: id.to_owned(),
        reason: reason.to_string(),
    }
}

/// A failure naming the command.
fn failed(id: &str, message: impl ToString) -> ShellError {
    ShellError::Failed {
        command: id.to_owned(),
        message: message.to_string(),
    }
}

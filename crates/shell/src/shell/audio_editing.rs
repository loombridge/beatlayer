//! Crop, Consolidate, and Consolidate Time to New Scene through the shell
//! (WP-4.13): the new files go through WP-4.20's processed-file writer, and
//! each edit lands as one undo step when its file is whole. The behaviour is
//! in the audio clip editing spec, R14 to R28.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, MidiData, SceneId, TrackId,
    TrackKind, WarpMarker, consolidated_midi,
};
use bl_shell_spec::audio_editing::{
    self as ids, ConsolidateResult, ConsolidateSceneArgs, ConsolidateSceneResult, CropArgs,
    CropResult, ReverseArgs, ReverseRangeArgs, ReverseRangeResult, ReverseResult,
};
use bl_shell_spec::editing::TargetArgs;
use serde_json::Value;

use super::*;
use crate::arrangement::editing;
use crate::arrangement::placing::clear_on;
use crate::processed::{Kind, Request, Written};

/// The longest a consolidate render may run: a day, as an export's is.
const LONGEST: f64 = 86_400.0;

/// How close two positions may be and still name the same point: a
/// millionth of a frame at 192 kHz.
const SLACK: f64 = 1e-9;

/// One track's consolidated clip: its place in the material the range
/// covered, the track, the new clip, and the file it plays.
type Landed = (usize, TrackId, ClipId, String);

/// A clip a reversal made: the clip, and where its new file is.
type Reversed = (ClipId, Option<String>);

impl Shell {
    /// Runs one of the audio clip editing commands (WP-4.13).
    pub(super) fn run_audio_editing(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::CROP => self.crop(id, args),
            ids::REVERSE => self.reverse(id, args),
            ids::ARRANGEMENT_REVERSE => self.arrangement_reverse(id, args),
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
            ids::REVERSE => match Self::args::<ReverseArgs>(id, args.clone()) {
                Ok(args) => self.reverse_target(&args).is_ok(),
                Err(_) => false,
            },
            ids::ARRANGEMENT_REVERSE => match Self::args::<ReverseRangeArgs>(id, args.clone()) {
                Ok(args) => self.reverse_range(id, &args).is_ok(),
                Err(_) => false,
            },
            // Consolidate works on take lanes too (R29); to a scene, on
            // main lanes only.
            ids::CONSOLIDATE => match Self::args::<TargetArgs>(id, args.clone()) {
                Ok(args) => self.lane_material_of(id, &args).is_ok(),
                Err(_) => false,
            },
            ids::CONSOLIDATE_TO_SCENE => match Self::args::<TargetArgs>(id, args.clone()) {
                Ok(args) => self.material_of(id, &args).is_ok(),
                Err(_) => false,
            },
            _ => false,
        }
    }

    /// `clip.reverse` (R7 to R10): the clip's file is written backwards
    /// through the processed-file writer, and the clip plays it.
    fn reverse(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: ReverseArgs = Self::args(id, args)?;
        let clip = self.reverse_target(&args).map_err(|why| invalid(id, why))?;
        let value = self.clip_of(id, clip)?;
        let ClipData::Audio(audio) = &value.data else {
            // Spec: R12. A MIDI clip isn't reversed, and nothing changes.
            return Ok(to_value(&ReverseResult {
                clip,
                media: None,
                path: None,
            }));
        };
        let audio = audio.clone();
        let decoded = self
            .edited_audio(audio.media, &[])
            .ok_or_else(|| failed(id, "the clip's file is missing"))?;
        let seconds = decoded.frames() as f64 / f64::from(decoded.sample_rate());
        // Spec: R10. A file Beatlayer made as a reversed copy goes back to
        // the one it was made from, writing nothing.
        if let Some(original) = self.store.reversed_from(&audio.media) {
            let command = Command::ReverseClip {
                clip,
                media: Some(original),
                seconds,
            };
            self.apply_one(id, "Reverse clip", command)?;
            return Ok(to_value(&ReverseResult {
                clip,
                media: Some(original),
                path: None,
            }));
        }
        let landed: Rc<RefCell<Option<(ContentHash, String)>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&landed);
        self.reverse_file(id, &audio, move |shell, written| {
            let hash = ContentHash(*written.hash.as_bytes());
            shell
                .editor
                .transact("Reverse clip", |editor| {
                    editor
                        .apply(Command::ReverseClip {
                            clip,
                            media: Some(hash),
                            seconds,
                        })
                        .map(drop)
                })
                .map_err(|error| error.to_string())?;
            shell.changed();
            *slot.borrow_mut() = Some((hash, written.path.to_string_lossy().into_owned()));
            Ok(())
        })?;
        self.wait_processed();
        let (media, path) = landed
            .borrow_mut()
            .take()
            .ok_or_else(|| failed(id, "the new file couldn't be written"))?;
        Ok(to_value(&ReverseResult {
            clip,
            media: Some(media),
            path: Some(path),
        }))
    }

    /// `arrangement.reverse` (R11, R12): the audio in a span of the
    /// Arrangement is reversed, across several clips, and the span plays
    /// backwards as a whole.
    fn arrangement_reverse(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: ReverseRangeArgs = Self::args(id, args)?;
        let (material, start, end) = self.reverse_range(id, &args)?;
        if material.is_empty() {
            return Err(unavailable(id, "no clip is in the selection"));
        }
        // Spec: R12. A selection that holds a MIDI clip isn't reversed, and
        // nothing changes.
        for (_, clips) in &material {
            for clip in clips {
                if !matches!(clip.data, ClipData::Audio(_)) {
                    return Err(unavailable(id, "the selection holds a MIDI clip"));
                }
            }
        }
        // Spec: freeze-and-bounce R8. A clip on a frozen track isn't
        // reversed.
        {
            let doc = self.editor.document();
            for (track, _) in &material {
                if doc.track(*track).is_some_and(|track| track.is_frozen()) {
                    return Err(unavailable(
                        id,
                        format!(
                            "the track {} is frozen; unfreeze it to change its clip settings",
                            track.0
                        ),
                    ));
                }
            }
        }
        let tracks: Vec<TrackId> = material.iter().map(|(track, _)| *track).collect();
        self.editor.begin("Reverse clip(s)");
        let outcome = self.reverse_span(id, &tracks, start, end);
        let done = match outcome {
            Ok(done) => {
                self.editor.commit().unwrap_or(false);
                self.changed();
                done
            }
            Err(error) => {
                self.editor.cancel().ok();
                return Err(error);
            }
        };
        Ok(to_value(&ReverseRangeResult {
            clips: done.iter().map(|(clip, _)| *clip).collect(),
            tracks,
            files: done.iter().filter_map(|(_, path)| path.clone()).collect(),
        }))
    }

    /// Splits the clips a span crosses at its edges, reverses each part
    /// inside, and moves the parts so the span plays backwards as a whole
    /// (R11).
    fn reverse_span(
        &mut self,
        id: &str,
        tracks: &[TrackId],
        start: Ticks,
        end: Ticks,
    ) -> Result<Vec<(ClipId, Option<String>)>, ShellError> {
        // The clips the span covers, right to left, so each keeps its first
        // part.
        for &track in tracks {
            let clips: Vec<ClipId> = self
                .editor
                .document()
                .track(track)
                .map(|track| {
                    track
                        .clips()
                        .filter(|clip| clip.start < end && clip.end() > start)
                        .map(|clip| clip.id)
                        .collect()
                })
                .unwrap_or_default();
            for clip in clips {
                for at in [end, start] {
                    let crosses = self
                        .editor
                        .document()
                        .clip(clip)
                        .is_some_and(|value| value.start < at && at < value.end());
                    if crosses {
                        crate::arrangement::placing::split_off(&mut self.editor, clip, at)
                            .map_err(|error| failed(id, error))?;
                    }
                }
            }
        }
        // The parts inside the span, each with where it lands: a part that
        // started *d* after the span's start ends *d* before its end.
        let mut parts: Vec<(TrackId, ClipId, Ticks)> = Vec::new();
        for &track in tracks {
            let inside: Vec<(ClipId, Ticks, Ticks)> = self
                .editor
                .document()
                .track(track)
                .map(|track| {
                    track
                        .clips()
                        .filter(|clip| clip.start >= start && clip.end() <= end)
                        .map(|clip| (clip.id, clip.start, clip.end()))
                        .collect()
                })
                .unwrap_or_default();
            for (clip, _from, to) in inside {
                let lands = start + (end - to);
                parts.push((track, clip, lands));
            }
        }
        let landed: Rc<RefCell<Vec<Reversed>>> = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::clone(&landed);
        for (track, clip, lands) in parts {
            let Some(value) = self.editor.document().clip(clip).cloned() else {
                continue;
            };
            let ClipData::Audio(audio) = &value.data else {
                continue;
            };
            let audio = audio.clone();
            let decoded = self
                .edited_audio(audio.media, &[])
                .ok_or_else(|| failed(id, "a clip's file is missing"))?;
            let seconds = decoded.frames() as f64 / f64::from(decoded.sample_rate());
            let slot = Rc::clone(&done);
            self.reverse_file(id, &audio, move |shell, written| {
                let hash = ContentHash(*written.hash.as_bytes());
                shell
                    .editor
                    .transact("Reverse clip(s)", |editor| {
                        editor
                            .apply(Command::ReverseClip {
                                clip,
                                media: Some(hash),
                                seconds,
                            })
                            .map(drop)
                            .and_then(|()| {
                                editor
                                    .apply(Command::MoveClip {
                                        clip,
                                        track,
                                        start: lands,
                                    })
                                    .map(drop)
                            })
                    })
                    .map_err(|error| error.to_string())?;
                shell.changed();
                slot.borrow_mut()
                    .push((clip, Some(written.path.to_string_lossy().into_owned())));
                Ok(())
            })
            .map_err(|error| failed(id, error))?;
        }
        self.wait_processed();
        Ok(landed.borrow().clone())
    }

    /// Writes the reversed copy of `audio`'s file through the processed-file
    /// writer (R7), and runs `done` with its hash when it is whole.
    fn reverse_file(
        &mut self,
        id: &str,
        audio: &AudioData,
        done: impl FnOnce(&mut Shell, &Written) -> Result<(), String> + 'static,
    ) -> Result<(), ShellError> {
        let decoded = self
            .edited_audio(audio.media, &[])
            .ok_or_else(|| failed(id, "the clip's file is missing"))?;
        let rate = decoded.sample_rate();
        let frames = decoded.frames();
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
        let original = audio.media;
        let request = Request {
            kind: Kind::Reverse,
            name: format!("{stem} Reverse.wav"),
            spec: bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: rate,
                channels: channels as u16,
            },
            frames: frames as u64,
            produce: Box::new(move |sink| {
                // Spec: R7. Frame n of the new file, in every channel, is
                // frame L - 1 - n of the source.
                let mut buffer = Vec::with_capacity(4096 * channels);
                let mut at = frames;
                while at > 0 {
                    let from = at.saturating_sub(4096);
                    buffer.clear();
                    for frame in (from..at).rev() {
                        for channel in 0..channels {
                            buffer.push(
                                decoded
                                    .channel(channel)
                                    .map_or(0.0, |samples| samples[frame]),
                            );
                        }
                    }
                    sink.write(&buffer)?;
                    at = from;
                }
                Ok(())
            }),
        };
        self.start_processed(request, move |shell, written| {
            let hash = ContentHash(*written.hash.as_bytes());
            // Spec: R10. The set's media table keeps which file the reversed
            // copy was made from.
            shell
                .store
                .add_reversed_media(hash, &written.path, written.size, original);
            done(shell, &written)
        })
        .map_err(|error| failed(id, error))?;
        Ok(())
    }

    /// The clip a `clip.reverse` acts on: the one the call names, the one
    /// the Clip View shows, or the one the Arrangement has selected.
    fn reverse_target(&self, args: &ReverseArgs) -> Result<ClipId, String> {
        let doc = self.editor.document();
        if let Some(clip) = args.clip {
            return doc
                .clip(clip)
                .map(|_| clip)
                .ok_or_else(|| format!("no clip has the ID {}", clip.0));
        }
        if let Some(shown) = self.clip_view.shown_any_clip(doc, &self.arrangement) {
            return Ok(shown);
        }
        self.arrangement
            .selected_clips()
            .first()
            .copied()
            .ok_or_else(|| "no clip is selected".to_owned())
    }

    /// The material a `arrangement.reverse` acts on: the clips a span of
    /// time covers, on the tracks it covers, and the span itself.
    #[expect(
        clippy::type_complexity,
        reason = "each track's clips cut to the range, and the range"
    )]
    fn reverse_range(
        &self,
        id: &str,
        args: &ReverseRangeArgs,
    ) -> Result<(Vec<(TrackId, Vec<Clip>)>, Ticks, Ticks), ShellError> {
        let doc = self.editor.document();
        let target = editing::target(
            doc,
            &self.arrangement,
            TargetArgs {
                clips: None,
                tracks: args.tracks.clone(),
                lanes: None,
                start: args.start.clone(),
                end: args.end.clone(),
                linked: Some(false),
            },
        )
        .map_err(|failure| edit_error(id, failure))?;
        let (by_track, start, end) = editing::material(doc, &target);
        let material: Vec<(TrackId, Vec<Clip>)> = by_track
            .into_iter()
            .filter(|(_, clips)| !clips.is_empty())
            .collect();
        Ok((material, start, end))
    }

    /// `clip.crop` (R14 to R18): an audio clip's region is written as a new
    /// file and it plays that; a MIDI clip loses the notes outside its region.
    fn crop(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: CropArgs = Self::args(id, args)?;
        let doc = self.editor.document();
        let shown = self.clip_view.shown_any_clip(doc, &self.arrangement);
        let span = crop_span(doc, &self.arrangement, &self.clip_view, shown, &args)
            .map_err(|why| invalid(id, why))?;
        let clip = crop_clip(doc, &self.arrangement, shown, span, &args)
            .ok_or_else(|| unavailable(id, "no clip is selected"))?;
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
        let doc = self.editor.document();
        let in_slot = doc
            .clip_track(clip)
            .and_then(|track| doc.track(track))
            .is_some_and(|track| track.slots().any(|(_, held)| held.id == clip));
        let command = match span {
            // A Session clip has no place in the Arrangement to move the
            // edges of, so it keeps the span's part of its contents.
            Some((start, end)) if in_slot => Command::CropClip {
                clip,
                media: None,
                from: (midi.start + (start - value.start)).get() as f64,
                to: (midi.start + (end - value.start)).get() as f64,
            },
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
        let (material, start, end) = self.lane_material_of(id, &args)?;
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
            lanes: None,
            start: args.start,
            end: args.end,
            linked: Some(false),
        };
        // A take lane's material is no scene's: only the main lanes.
        let (material, start, end) = self.material_of(id, &targets)?;
        let material = material
            .into_iter()
            .map(|(track, clips)| (editing::Place::main(track), clips))
            .collect();
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
        material: Vec<(editing::Place, Vec<Clip>)>,
        start: Ticks,
        end: Ticks,
        scene: Option<SceneId>,
    ) -> Result<Vec<(TrackId, ClipId, Option<String>)>, ShellError> {
        let landed: Rc<RefCell<Vec<Landed>>> = Rc::new(RefCell::new(Vec::new()));
        let broke: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let mut order: Vec<(usize, TrackId, ClipId, Option<String>)> = Vec::new();
        for (index, entry) in material.iter().enumerate() {
            let lane = entry.0;
            let track = lane.track;
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
                    let look = &self.prefs.look;
                    let placed = self.editor.transact(label_of(scene), |editor| {
                        place(editor, lane, scene, start, end, clip)?;
                        crate::colours::colour_new(editor, look, &[], &[new_id])
                    });
                    match placed {
                        Ok(()) => order.push((index, track, new_id, None)),
                        Err(error) => return Err(failed(id, error)),
                    }
                }
                Some(TrackKind::Audio) => {
                    let (frames, channels, gain, markers, tempo) =
                        self.consolidate_audio(id, lane, start, end)?;
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
                    // Spec: scales R7. A consolidated clip keeps the scale
                    // of the first clip it was made from.
                    let scale = clips
                        .first()
                        .map_or_else(bl_model::Scale::default, |clip| clip.scale);
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
                            scale,
                            // The render plays the groove, so it holds it.
                            groove: None,
                            colour: None,
                            info_text: String::new(),
                        };
                        let look = &shell.prefs.look;
                        let placed = shell.editor.transact(label_of(scene), |editor| {
                            place(editor, lane, scene, start, end, clip)?;
                            crate::colours::colour_new(editor, look, &[], &[new_id])
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
        place: editing::Place,
        start: Ticks,
        end: Ticks,
    ) -> Result<(Vec<f32>, u16, f64, Vec<WarpMarker>, f64), ShellError> {
        let document = self.editor.document();
        let track = place.track;
        // A take lane's clips play when it's auditioned (the comping spec,
        // R14): a lane is consolidated by rendering the track so.
        let prepared =
            consolidate_document(document, track, place.lane).map_err(|why| failed(id, why))?;
        let rate = self.engine_rate();
        let media = self.engine_media();
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

    /// What a consolidate acts on: each lane (a track's main lane or one of
    /// its take lanes, the comping spec R27) with the clips the range covers,
    /// already cut to the range, and the range itself.
    #[expect(
        clippy::type_complexity,
        reason = "each lane's clips cut to the range, and the range"
    )]
    fn lane_material_of(
        &self,
        id: &str,
        args: &TargetArgs,
    ) -> Result<(Vec<(editing::Place, Vec<Clip>)>, Ticks, Ticks), ShellError> {
        let doc = self.editor.document();
        let target = editing::target(doc, &self.arrangement, args.clone())
            .map_err(|failure| edit_error(id, failure))?;
        let (by_place, start, end) = editing::lane_material(doc, &target);
        let material: Vec<(editing::Place, Vec<Clip>)> = by_place
            .into_iter()
            .filter(|(_, clips)| !clips.is_empty())
            .collect();
        if material.is_empty() {
            return Err(unavailable(id, "no clip is in the range"));
        }
        Ok((material, start, end))
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

    /// The clip a `clip.crop` acts on: the one it names, the one the Clip
    /// View shows, or the clip the span covers. The menus' enabled states
    /// read this (R28).
    fn crop_target(&self, args: &Value) -> Result<ClipId, ShellError> {
        let args: CropArgs = Self::args(ids::CROP, args.clone())?;
        let doc = self.editor.document();
        let shown = self.clip_view.shown_any_clip(doc, &self.arrangement);
        let span = crop_span(doc, &self.arrangement, &self.clip_view, shown, &args)
            .map_err(|why| invalid(ids::CROP, why))?;
        crop_clip(doc, &self.arrangement, shown, span, &args)
            .ok_or_else(|| unavailable(ids::CROP, "no clip is selected"))
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
    lane: editing::Place,
    scene: Option<SceneId>,
    start: Ticks,
    end: Ticks,
    clip: Clip,
) -> Result<(), bl_model::EditError> {
    // Spec: scales R7. A consolidated clip keeps the scale of the first
    // clip it was made from, which the clip takes the set's current scale
    // when it is made.
    let scale = clip.scale;
    let id = clip.id;
    let track = lane.track;
    if let Some(scene) = scene {
        editor.apply(Command::CreateSlotClip { track, scene, clip })?;
    } else {
        clear_on(editor, track, lane.lane, start, end, &BTreeSet::new())?;
        editor.apply(match lane.lane {
            Some(lane) => Command::CreateTakeClip { lane, clip },
            None => Command::CreateClip { track, clip },
        })?;
    }
    if scale != editor.document().settings().scale {
        editor.apply(scale.command(vec![id]))?;
    }
    Ok(())
}

/// The span a crop keeps (R16): the call's `start` and `end`, or, when it
/// names neither a clip nor a span, the Arrangement's time selection, which
/// the clip and time selection menus and Cmd-Shift-J run with. `shown` is
/// the clip the Clip View shows, which a selection-less call crops instead.
/// The permission assessment resolves the span here too, so the two agree on
/// what a call would touch.
///
/// # Errors
///
/// The message an unresolvable span fails with: one of `start` and `end`
/// without the other, or an `end` not after its `start`.
pub(crate) fn crop_span(
    doc: &Document,
    view: &ArrangementView,
    clip_view: &crate::clip_view::ClipView,
    shown: Option<ClipId>,
    args: &CropArgs,
) -> Result<Option<(Ticks, Ticks)>, String> {
    match (&args.start, &args.end) {
        (Some(start), Some(end)) => {
            let signatures = &doc.settings().time_signatures;
            let start = start.resolve(signatures).map_err(|why| why.to_string())?;
            let end = end.resolve(signatures).map_err(|why| why.to_string())?;
            if end <= start {
                return Err("the span's `end` must be after its `start`".to_owned());
            }
            Ok(Some((start, end)))
        }
        (None, None) if args.clip.is_none() && shown.is_none() => Ok(view.time_selection()),
        (None, None) => editor_span(doc, view, clip_view, args.clip.or(shown)),
        _ => Err("give `start` and `end` together".to_owned()),
    }
}

/// The span a crop keeps when the note editor shows the clip and has a time
/// selection (piano-roll R40), in song positions: the Arrangement's own time
/// selection comes first when it lies inside the clip.
fn editor_span(
    doc: &Document,
    view: &ArrangementView,
    clip_view: &crate::clip_view::ClipView,
    clip: Option<ClipId>,
) -> Result<Option<(Ticks, Ticks)>, String> {
    let Some(value) = clip.and_then(|id| doc.clip(id)) else {
        return Ok(None);
    };
    let ClipData::Midi(midi) = &value.data else {
        return Ok(None);
    };
    let Some((from, to)) = clip_view
        .time_selection(doc, view)
        .filter(|_| clip_view.shown_clip(doc, view) == Some(value.id))
    else {
        return Ok(None);
    };
    if let Some((start, end)) = view
        .time_selection()
        // Selecting a clip gives the Arrangement a time selection as long as
        // the clip, which is the clip and not a time of its own.
        .filter(|&(start, end)| {
            start >= value.start && end <= value.end() && (start, end) != (value.start, value.end())
        })
    {
        return Ok(Some((start, end)));
    }
    let start = (value.start + (from - midi.start)).max(value.start);
    let end = (value.start + (to - midi.start)).min(value.end());
    if start >= end {
        return Err("the time selection lies outside the clip".to_owned());
    }
    Ok(Some((start, end)))
}

/// The clip a crop acts on (R16): the one the call names, the clip the Clip
/// View shows, or the first clip the span covers on the selected tracks.
#[must_use]
pub(crate) fn crop_clip(
    doc: &Document,
    view: &ArrangementView,
    shown: Option<ClipId>,
    span: Option<(Ticks, Ticks)>,
    args: &CropArgs,
) -> Option<ClipId> {
    if let Some(clip) = args.clip {
        return doc.clip(clip).map(|_| clip);
    }
    if let Some(clip) = shown.filter(|id| doc.clip(*id).is_some()) {
        return Some(clip);
    }
    let (start, end) = span?;
    // Spec: comping R27. The lanes the selection is on, take lanes included.
    let places = if view.selected_tracks().is_empty() {
        doc.track_order()
            .into_iter()
            .map(editing::Place::main)
            .collect()
    } else {
        editing::in_row_order(doc, view.selected_places())
    };
    places.into_iter().find_map(|place| {
        place
            .clips(doc)
            .into_iter()
            .find(|clip| clip.start < end && clip.end() > start)
            .map(|clip| clip.id)
    })
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
        fade_in_curve: 0.0,
        fade_out_curve: 0.0,
        fade_in_before: 0.0,
        fade_out_after: 0.0,
        warp: true,
        warp_markers: markers,
        tempo: Some(tempo),
        signature: None,
        warp_mode: bl_model::WarpMode::Beats,
        warp_settings: bl_model::WarpSettings::default(),
        tempo_leader: false,
        ram: false,
        high_quality: true,
        transpose: 0,
        detune: 0.0,
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
/// another track's. Slice to New MIDI Track renders through it too
/// (slice-to-midi R6).
///
/// # Errors
///
/// A message if the document refuses one of the steps.
pub(super) fn consolidate_document(
    document: &Document,
    track: TrackId,
    lane: Option<bl_model::TakeLaneId>,
) -> Result<Document, String> {
    let mut doc = document.clone();
    if let Some(lane) = lane {
        doc.apply(&Command::SetAudition { lane, on: true })
            .map_err(|error| error.to_string())?;
    }
    let main = doc.main_id();
    // Spec: R20. The track's clips reach Main alone: a track in a group
    // comes out of it before the group goes, or deleting the group would
    // take the track along with it.
    if doc.parent(track).is_some() {
        doc.apply(&Command::MoveTrack {
            track,
            parent: None,
            index: 0,
        })
        .map_err(|error| error.to_string())?;
    }
    let others: Vec<TrackId> = doc
        .track_order()
        .into_iter()
        .filter(|id| *id != track && *id != main)
        .collect();
    for other in others {
        doc.apply(&Command::DeleteTrack { track: other })
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

//! Freezing and unfreezing tracks through the shell (WP-4.14): the render
//! goes through the export's offline engine and the files through WP-4.20's
//! processed-file writer, and the edit lands as one undo step when every
//! file is whole. The behaviour is in the freeze and bounce spec, R1 to
//! R17 and R27 to R29.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, ContentHash, Document, FrozenClip, FrozenTail, FrozenTrack,
    SceneId, TrackId, TrackKind,
};
use bl_shell_spec::freeze::{self as ids, FreezeArgs, FreezeResult};
use serde_json::Value;

use super::*;
use crate::export::Length;
use crate::processed::{Kind, Request, Written};

/// The longest a freeze render may run: a day, as an export's is.
const LONGEST: f64 = 86_400.0;

/// A frame at or below this is silence for a tail's end: −120 dBFS.
const FLOOR: f32 = 1e-6;

impl Shell {
    /// Runs one of the freeze commands (WP-4.14).
    pub(super) fn run_freeze(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: FreezeArgs = Self::args(id, args)?;
        match id {
            ids::FREEZE => self.freeze(args),
            ids::UNFREEZE => self.unfreeze(id, args),
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether a freeze command can run with `args`: the menu's enabled
    /// state (R28).
    pub(super) fn can_freeze(&self, id: &str, args: &Value) -> bool {
        let Ok(args) = Self::args::<FreezeArgs>(id, args.clone()) else {
            return false;
        };
        let tracks = self.freeze_targets(&args);
        let doc = self.editor.document();
        if id == ids::UNFREEZE {
            return tracks
                .iter()
                .any(|track| doc.track(*track).is_some_and(|t| t.is_frozen()));
        }
        tracks.iter().any(|track| {
            doc.track(*track).is_some_and(|t| {
                t.kind() != TrackKind::Main
                    && (!t.kind().holds_clips() || t.clips().len() > 0 || t.slots().len() > 0)
            })
        })
    }

    /// `track.freeze` (R1 to R17). When every named track is frozen it
    /// unfreezes them, as the key does (R1).
    fn freeze(&mut self, args: FreezeArgs) -> Result<Value, ShellError> {
        let id = ids::FREEZE;
        let tracks = self.freeze_targets(&args);
        if tracks.is_empty() {
            return Err(unavailable(id, "no track is selected"));
        }
        let doc = self.editor.document();
        if tracks
            .iter()
            .all(|track| doc.track(*track).is_some_and(|t| t.is_frozen()))
        {
            return self.unfreeze_tracks(id, tracks);
        }
        let mut frozen = Vec::new();
        let mut files = Vec::new();
        for track in tracks {
            let (audio, written) = self.freeze_track(id, track)?;
            frozen.push((track, audio));
            files.extend(written);
        }
        self.editor
            .transact("Freeze tracks", |editor| {
                editor.apply(Command::FreezeTracks { frozen }).map(drop)
            })
            .map_err(|error| failed(id, error.to_string()))?;
        self.changed();
        Ok(to_value(&FreezeResult {
            frozen: self
                .editor
                .document()
                .tracks()
                .filter(|track| track.is_frozen())
                .map(|track| track.id())
                .collect(),
            files,
        }))
    }

    /// `track.unfreeze` (R10).
    fn unfreeze(&mut self, id: &str, args: FreezeArgs) -> Result<Value, ShellError> {
        let tracks = self.freeze_targets(&args);
        if tracks.is_empty() {
            return Err(unavailable(id, "no track is selected"));
        }
        self.unfreeze_tracks(id, tracks)
    }

    fn unfreeze_tracks(&mut self, id: &str, tracks: Vec<TrackId>) -> Result<Value, ShellError> {
        let frozen: Vec<TrackId> = tracks
            .iter()
            .copied()
            .filter(|track| {
                self.editor
                    .document()
                    .track(*track)
                    .is_some_and(|t| t.is_frozen())
            })
            .collect();
        if frozen.is_empty() {
            return Err(unavailable(id, "no track is frozen"));
        }
        self.editor
            .transact("Unfreeze tracks", |editor| {
                editor
                    .apply(Command::UnfreezeTracks {
                        tracks: frozen.clone(),
                    })
                    .map(drop)
            })
            .map_err(|error| failed(id, error.to_string()))?;
        self.changed();
        Ok(to_value(&FreezeResult {
            frozen: Vec::new(),
            files: Vec::new(),
        }))
    }

    /// The tracks a freeze acts on: those named, or the selected tracks.
    fn freeze_targets(&self, args: &FreezeArgs) -> Vec<TrackId> {
        args.tracks.clone().unwrap_or_else(|| {
            self.arrangement
                .selected_tracks()
                .iter()
                .copied()
                .filter(|track| {
                    self.editor
                        .document()
                        .track(*track)
                        .is_some_and(|t| t.kind() != TrackKind::Main)
                })
                .collect()
        })
    }

    /// Renders `track` and writes its files, returning what to record and
    /// where the files went.
    fn freeze_track(
        &mut self,
        id: &str,
        track: TrackId,
    ) -> Result<(FrozenTrack, Vec<String>), ShellError> {
        let doc = self.editor.document().clone();
        let held = doc
            .track(track)
            .ok_or_else(|| failed(id, "no such track"))?;
        if held.kind() == TrackKind::Main {
            return Err(unavailable(id, "Main can't be frozen"));
        }
        if held.kind().holds_clips() && held.clips().len() == 0 && held.slots().len() == 0 {
            return Err(unavailable(id, "a track without clips can't be frozen"));
        }
        let label = crate::routing::track_label(&doc, track);
        let arrangement = arrangement_bounds(held);
        let mut frozen = FrozenTrack::default();
        let mut files = Vec::new();
        match held.kind() {
            TrackKind::Audio | TrackKind::Midi => {
                if let Some((start, end)) = arrangement {
                    let (frames, rate) = self.render_frozen(id, track, start, end, true)?;
                    let channels = channels_of(&frames);
                    let (_start_sec, end_sec) = (
                        0.0,
                        frames.len() as f64 / f64::from(channels) / f64::from(rate),
                    );
                    let mut clips: Vec<&Clip> = held.clips().collect();
                    clips.sort_by_key(|clip| (clip.start, clip.id));
                    let mut regions = Vec::new();
                    let mut tails = Vec::new();
                    for (index, clip) in clips.iter().enumerate() {
                        let from = seconds_between(&doc, start, clip.start);
                        let to = seconds_between(&doc, start, clip.start + clip.length);
                        regions.push((clip.id, clip.start, from, to));
                        // R3: a tail runs from the clip's end to the next
                        // clip's start, or until the render falls silent.
                        let next = clips.get(index + 1).map_or(end_sec, |next| {
                            seconds_between(&doc, start, next.start).min(end_sec)
                        });
                        let tail_end = quiet_end(&frames, channels, rate, to, next);
                        if tail_end > to + 1e-9 {
                            tails.push((clip.id, clip.start + clip.length, to, tail_end));
                        }
                    }
                    let media = self.write_frozen(
                        id,
                        Kind::Freeze,
                        &format!("{label} Freeze [{}].wav", crate::recording::stamp()),
                        frames,
                        channels,
                        rate,
                        &mut files,
                    )?;
                    let tempo = doc.settings().tempo.bpm_at(start).get();
                    for (clip, _, from, to) in regions {
                        let held_clip = held.clips().find(|held| held.id == clip);
                        let length = held_clip.map_or(Ticks::ZERO, |held| held.length);
                        let original = held_clip.map_or_else(
                            || bl_model::ClipData::Midi(bl_model::MidiData::default()),
                            |held| held.data.clone(),
                        );
                        frozen.clips.push(FrozenClip {
                            clip,
                            media,
                            start: from,
                            end: to,
                            tempo,
                            length,
                            original,
                            scene: None,
                            cycle: None,
                        });
                    }
                    for (clip, start, file_start, file_end) in tails {
                        let length = frames_to_ticks(&doc, rate, file_end - file_start);
                        frozen.tails.push(FrozenTail {
                            clip,
                            id: self.tail_id(frozen.tails.len(), clip),
                            start,
                            length,
                            media,
                            file_start,
                            file_end,
                        });
                    }
                    for (scene, clip) in held.slots() {
                        let (session_media, start_sec, end_sec, cycle) =
                            self.freeze_session_clip(id, track, scene, clip, &mut files)?;
                        frozen.clips.push(FrozenClip {
                            clip: clip.id,
                            media: session_media,
                            start: start_sec,
                            end: end_sec,
                            tempo: doc.settings().tempo.bpm_at(Ticks::ZERO).get(),
                            length: clip.length,
                            original: clip.data.clone(),
                            scene: Some(scene),
                            cycle,
                        });
                    }
                }
            }
            TrackKind::Group | TrackKind::Return => {
                let start = Ticks::ZERO;
                let end = arrangement_end(&doc);
                let (frames, rate) = self.render_frozen(id, track, start, end, false)?;
                let channels = channels_of(&frames);
                let media = self.write_frozen(
                    id,
                    Kind::Freeze,
                    &format!("{label} Freeze [{}].wav", crate::recording::stamp()),
                    frames,
                    channels,
                    rate,
                    &mut files,
                )?;
                frozen.output = Some(bl_model::FrozenOutput {
                    id: self.editor.new_id(),
                    media,
                    tempo: doc.settings().tempo.bpm_at(start).get(),
                    start,
                    length: end,
                });
            }
            TrackKind::Main => {}
        }
        Ok((frozen, files))
    }

    /// The ID a tail region takes: the track's freeze has no document IDs
    /// of its own, so tails count from the top of the ID space down, under a
    /// number no clip uses.
    fn tail_id(&self, _index: usize, clip: ClipId) -> ClipId {
        clip
    }

    /// Renders a frozen Session clip alone, from its Start marker to the
    /// end of its second turn round the loop (R4), and writes its file.
    fn freeze_session_clip(
        &mut self,
        id: &str,
        track: TrackId,
        _scene: SceneId,
        clip: &Clip,
        files: &mut Vec<String>,
    ) -> Result<(ContentHash, f64, f64, Option<f64>), ShellError> {
        let doc = self.editor.document().clone();
        let mut prepared = prepared_track(&doc, track).map_err(|why| failed(id, why))?;
        // The track plays only this clip, from 1.1.1: its Arrangement clips
        // and the other slots come out.
        let others: Vec<ClipId> = prepared
            .track(track)
            .map(|t| {
                t.clips()
                    .map(|clip| clip.id)
                    .chain(t.slots().map(|(_, clip)| clip.id))
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
            .and_then(|t| t.slots().next())
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
        let channels = channels_of(&frames);
        let seconds = frames.len() as f64 / f64::from(channels) / f64::from(rate);
        let label = crate::routing::track_label(&doc, track);
        let cycle =
            (turns == 2).then(|| seconds_between(&doc, Ticks::ZERO, Ticks::ZERO + clip.length));
        let media = self.write_frozen(
            id,
            Kind::Freeze,
            &format!("{label} Freeze [{}].wav", crate::recording::stamp()),
            frames,
            channels,
            rate,
            files,
        )?;
        Ok((media, 0.0, seconds, cycle))
    }

    /// Renders `track` from `start` to `end`, with its tail when asked, at
    /// the engine's rate, mono when its two channels are equal (R5).
    fn render_frozen(
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

    /// Writes one freeze file through the processed-file writer.
    #[expect(
        clippy::too_many_arguments,
        reason = "the file's name, audio, format and result"
    )]
    fn write_frozen(
        &mut self,
        id: &str,
        kind: Kind,
        name: &str,
        mut frames: Vec<f32>,
        channels: u16,
        rate: u32,
        files: &mut Vec<String>,
    ) -> Result<ContentHash, ShellError> {
        // The render is stereo; a mono file takes one of each frame's
        // equal channels (R5).
        if channels == 1 {
            frames = frames.iter().copied().step_by(2).collect();
        }
        let total = frames.len() as u64 / u64::from(channels);
        let request = Request {
            kind,
            name: name.to_owned(),
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
            .ok_or_else(|| failed(id, "the freeze file couldn't be written"))?;
        files.push(written.path.to_string_lossy().into_owned());
        Ok(ContentHash(*written.hash.as_bytes()))
    }
}

/// The track's clips' first start and last end, if it has any.
fn arrangement_bounds(track: &bl_model::Track) -> Option<(Ticks, Ticks)> {
    let mut clips = track.clips();
    let first = clips.next()?;
    let mut start = first.start;
    let mut end = first.end();
    for clip in clips {
        start = start.min(clip.start);
        end = end.max(clip.end());
    }
    Some((start, end))
}

/// The end of the last clip in the set, for a return's render (R15).
fn arrangement_end(doc: &Document) -> Ticks {
    let mut end = Ticks::ZERO;
    for track in doc.tracks() {
        for clip in track.clips() {
            end = end.max(clip.end());
        }
    }
    end
}

/// A copy of `doc` that renders `track` Post FX: the export's preparation
/// (so a track inside a group leaves it, a return keeps what is sent to it,
/// Main's devices are off at unity, and every send and return around the
/// track is silenced), with the track's own mixer at rest, since its volume,
/// pan, activator and sends act on the render afterwards (R2, R14, R15).
fn prepared_track(doc: &Document, track: TrackId) -> Result<Document, String> {
    let mut doc = crate::export::prepare_document_for_track(doc, track, false)?;
    for command in [
        Command::SetVolume { track, volume: 1.0 },
        Command::SetPan { track, pan: 0.0 },
        Command::SetMute { track, mute: false },
    ] {
        doc.apply(&command).map_err(|error| error.to_string())?;
    }
    Ok(doc)
}

/// Renders `doc` from `start` for `length` ticks, with its tail when asked.
fn render_doc(
    shell: &Shell,
    id: &str,
    doc: &Document,
    start: Ticks,
    length: Ticks,
    tail: bool,
) -> Result<(Vec<f32>, u32), ShellError> {
    let rate = shell.engine_rate();
    let media: Arc<dyn bl_engine::MediaLibrary> = shell.media.clone();
    let spec = crate::export::Render {
        start,
        length: Length::To(start + length),
        sample_rate: rate,
        engine_rate: rate,
        block: crate::export::BLOCK,
    };
    let cancel = AtomicBool::new(false);
    let mut rendered: Vec<f32> = Vec::new();
    let mut tailed: Vec<f32> = Vec::new();
    {
        let mut sink = |samples: &[f32]| {
            rendered.extend_from_slice(samples);
            Ok(())
        };
        let mut tail_sink = |samples: &[f32]| {
            tailed.extend_from_slice(samples);
            Ok(())
        };
        let tail = tail.then_some(&mut tail_sink as &mut crate::export::Sink<'_>);
        crate::export::render(
            doc,
            media,
            shell.plugin_hosts(),
            &spec,
            LONGEST,
            &cancel,
            &mut |_| {},
            &mut sink,
            tail,
        )
        .map_err(|error| failed(id, error))?;
    }
    rendered.extend_from_slice(&tailed);
    if rendered.is_empty() {
        return Err(unavailable(id, "the track plays no audio"));
    }
    Ok((rendered, rate))
}

/// Whether the render's channels are equal throughout, and the samples as
/// one channel when they are (R5).
fn channels_of(frames: &[f32]) -> u16 {
    let equal = frames
        .as_chunks::<2>()
        .0
        .iter()
        .all(|frame| frame[0].to_bits() == frame[1].to_bits());
    if equal { 1 } else { 2 }
}

/// The second `at` sits at, counting from `from` through the tempo map.
fn seconds_between(doc: &Document, from: Ticks, at: Ticks) -> f64 {
    let rate = bl_core::time::SampleRate::new(48_000).expect("48 kHz is a rate");
    let frames = doc.settings().tempo.ticks_to_frames(at, rate).get()
        - doc.settings().tempo.ticks_to_frames(from, rate).get();
    frames as f64 / f64::from(rate.hz())
}

/// Ticks covering `seconds` at the set's tempo at 1.1.1, near enough for a
/// tail's drawn length.
fn frames_to_ticks(doc: &Document, rate: u32, seconds: f64) -> Ticks {
    let rate = bl_core::time::SampleRate::new(rate).expect("a known rate");
    let frames = (seconds * f64::from(rate.hz())).round() as i64;
    doc.settings()
        .tempo
        .frames_to_ticks(bl_core::time::Frames(frames), rate)
}

/// Where a tail falls silent: the last frame above [`FLOOR`] between `from`
/// and `to` seconds, in seconds; `to` or `from` when there is none.
fn quiet_end(frames: &[f32], channels: u16, rate: u32, from: f64, to: f64) -> f64 {
    let channels = usize::from(channels);
    let first = (from * f64::from(rate)).floor().max(0.0) as usize * channels;
    let last = ((to * f64::from(rate)).ceil() as usize * channels).min(frames.len());
    let mut end = from;
    for index in (first..last).step_by(channels) {
        if frames.get(index).is_some_and(|sample| sample.abs() > FLOOR) {
            end = index as f64 / f64::from(rate) / f64::from(channels as u32);
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

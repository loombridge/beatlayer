//! Recording: beginning it, polling it and placing the takes.

use super::{Shell, ShellError, edit_error, file_stem};
use crate::audio::EngineTransport;
use crate::auto_warp;
use crate::clip_view;
use bl_core::time::Frames;
use bl_core::time::SampleRate;
use bl_model::AudioData;
use bl_model::Clip;
use bl_model::ClipData;
use bl_model::ContentHash;
use bl_model::Editor;
use bl_model::Input;
use bl_model::TrackId;
use bl_model::TrackKind;
use bl_project::ProjectFolder;
use bl_shell_spec::editing as edit_ids;
use bl_shell_spec::notes as note_ids;
use bl_shell_spec::transport::RecordedFileReport;
use bl_shell_spec::transport::RecordingReport;
use bl_shell_spec::transport::RunningRecording;
use serde_json::Value;
use std::path::PathBuf;

impl Shell {
    /// Starts a recording (WP-2.13, WP-2.14): a ring for every armed audio
    /// track whose Audio From is Ext. In, and a writer thread draining them
    /// into the project's `Samples/Recorded/`; and a ring for every armed
    /// MIDI track, which needs no folder. The engine is given the rings
    /// before the transport is told it records.
    pub(super) fn begin_recording(&mut self) -> Result<(), String> {
        let rate = self
            .audio
            .sample_rate()
            .and_then(SampleRate::new)
            .ok_or("no audio device is open")?;
        let folder = self
            .set_path
            .as_ref()
            .map(|set| ProjectFolder::of_set(set).root().to_path_buf());
        let stamp = crate::recording::stamp();
        self.recorded_takes += 1;
        let take = self.recorded_takes;
        let document = self.editor.document();
        let mut tracks = Vec::new();
        let mut midi = Vec::new();
        for id in document.track_order() {
            let Some(track) = document.track(id) else {
                continue;
            };
            if !track.mixer().arm {
                continue;
            }
            match (track.kind(), &track.routing().input) {
                (TrackKind::Midi, _) => midi.push(id),
                (TrackKind::Audio, Input::External { channel }) => {
                    tracks.push(crate::recording::Track {
                        track: id,
                        channel: *channel,
                        name: crate::recording::file_name(
                            &crate::routing::track_label(document, id),
                            take,
                            &stamp,
                        ),
                    });
                }
                _ => {}
            }
        }
        if folder.is_none() && !tracks.is_empty() {
            // The audio has nowhere to go; the MIDI still records.
            self.set_message(
                "the set has never been saved; use `set.save_as` to give it a project folder, \
                 where its recordings go",
                true,
            );
            tracks.clear();
        }
        let (mut plan, mut recording) =
            crate::recording::start(folder.as_deref(), rate, &tracks, &midi, Vec::new())?;
        self.session_watch = crate::session_capture::Watch::default();
        let delay = self
            .audio
            .recording_delay()
            .ok_or("no audio device is open")?;
        plan.delay = delay;
        recording.delay = delay;
        self.audio.record(plan)?;
        self.recording = Some(recording);
        Ok(())
    }

    /// Follows the recording under way (WP-2.13): notes the take the engine
    /// published, and, once the writer has finished draining the rings,
    /// makes its clips. Called with every engine poll, so the clips appear
    /// a frame or two after the take ends.
    pub(super) fn poll_recording(&mut self, engine: &EngineTransport) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };
        if let Some(take) = engine.take {
            recording.note_take(take);
        }
        if let Some(outcome) = recording.poll(engine.running) {
            self.recording = None;
            self.finish_recording(outcome);
        } else {
            self.grow_session_clips(engine);
        }
    }

    /// Writes the control moves of an automation recording into the set
    /// when the transport stops recording (automation R17).
    pub(super) fn poll_automation_record(&mut self, engine: &EngineTransport) {
        let recording = engine.recording && engine.playing;
        let was = std::mem::replace(&mut self.automation_recording, recording);
        if recording {
            if !was {
                // The recording starts: the take learns the mode and where
                // it began (automation R18 to R21).
                let document = self.editor.document();
                let settings = document.settings();
                self.automation_take.start(
                    self.prefs.automation_record_mode,
                    crate::automation_record::take_start(document, engine.position),
                    settings.arrangement_loop.start,
                );
            }
            self.automation_take.advance(engine.position);
            return;
        }
        if !was {
            return;
        }
        let take = std::mem::take(&mut self.automation_take);
        match take.commit(&mut self.editor, engine.position) {
            Ok(true) => {
                self.dirty = true;
                self.document_changed();
                self.touch();
            }
            Ok(false) => {}
            Err(error) => self.set_message(format!("Automation recording failed: {error}"), true),
        }
    }

    /// Makes the clips of a finished recording, one undo step (WP-2.13,
    /// WP-2.14): each file becomes an audio clip on its track, and each
    /// armed MIDI track's notes a MIDI clip, from the song frame the take
    /// started on to the one it ended on. Undo removes the clips; the files
    /// stay, as Live keeps recorded files. A report the writer
    /// sent (a failed file) makes no clips and shows in the Status Bar, and
    /// frames the engine dropped because the disk fell behind are reported
    /// there too.
    fn finish_recording(&mut self, outcome: crate::recording::Outcome) {
        let crate::recording::Outcome {
            take,
            delay,
            files,
            midi,
            dropped,
            launches,
        } = outcome;
        self.last_recording = Some(RecordingReport {
            error: None,
            clips_made: false,
            files: files.as_ref().map_or_else(
                |_| Vec::new(),
                |files| {
                    files
                        .iter()
                        .map(|file| RecordedFileReport {
                            track: file.track,
                            path: file.path.display().to_string(),
                            frames: file.frames,
                            channels: file.channels,
                        })
                        .collect()
                },
            ),
            midi_tracks: midi.iter().map(|take| take.track).collect(),
            dropped_frames: dropped,
            delay_frames: delay,
        });
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                self.abandon_session_recording();
                self.fail_recording(format!("Recording failed: {error}"));
                return;
            }
        };
        if let Some(run) = self.session_run.take() {
            // Spec: session-recording R7. The clips go into slots.
            self.finish_session_recording(
                run,
                crate::recording::Outcome {
                    take,
                    delay,
                    files: Ok(files),
                    midi,
                    dropped,
                    launches,
                },
            );
            return;
        }
        if files.is_empty() && midi.is_empty() && launches.is_empty() {
            return;
        }
        let Some(take) = take else {
            self.fail_recording("Recording stopped without a position, so its clips weren't made");
            return;
        };
        let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new) else {
            self.fail_recording("Recording stopped: the audio device closed");
            return;
        };
        let tempo = self.editor.document().settings().tempo.clone();
        // Spec: recording R7. The audio is moved `delay` frames earlier; a
        // negative delay starts it later, where the file's first sample is.
        let start = tempo.frames_to_ticks(take.start + Frames((-delay).max(0)), rate);
        let end = tempo.frames_to_ticks(take.end - Frames(delay), rate);
        let edge_fades = self.create_fades_on_clip_edges();
        let warp_recorded = self.warp_recorded_audio();
        let warp_mode = auto_warp::warp_mode(self.prefs.warp.default_warp_mode);
        let overdub = self.editor.document().settings().midi_overdub;
        // shortcut: a take that went round the loop copies no Session
        // clips; upgrade when Live's behaviour there is observed.
        let watch = std::mem::take(&mut self.session_watch);
        let runs = if take.wrap.is_none() {
            crate::session_capture::runs(&launches, take.end, &tempo, rate)
        } else {
            Vec::new()
        };
        let mut imported = Vec::with_capacity(files.len());
        for file in &files {
            match self.media.import(&file.path) {
                Ok(hash) => {
                    let size = std::fs::metadata(&file.path).map_or(0, |data| data.len());
                    imported.push((
                        file.track,
                        ContentHash(*hash.as_bytes()),
                        file.path.clone(),
                        size,
                    ));
                }
                Err(error) => {
                    self.fail_recording(format!(
                        "Recording failed: {}: {error}",
                        file.path.display()
                    ));
                    return;
                }
            }
        }
        let take_lanes_before: Vec<(TrackId, usize)> = self
            .editor
            .document()
            .tracks()
            .map(|track| (track.id(), track.take_lanes().len()))
            .collect();
        let applied = match take.wrap {
            None => {
                let mut placed = std::collections::BTreeSet::new();
                let label = if runs.is_empty() {
                    crate::recording::UNDO_LABEL
                } else {
                    crate::recording::LOOP_UNDO_LABEL
                };
                let armed: Vec<TrackId> = imported
                    .iter()
                    .map(|(track, ..)| *track)
                    .chain(midi.iter().map(|take| take.track))
                    .collect();
                let before = self.editor.document().clone();
                let look = &self.prefs.look;
                self.editor.transact(label, |editor| {
                    for (track, hash, path, _) in &imported {
                        let id = editor.new_id();
                        // Spec: audio-clips R7. With Create Fades on Clip Edges
                        // on, a new recorded clip's edges fade over 4 ms, like a
                        // dropped file's.
                        let audio = AudioData::new(*hash);
                        // Spec: warping R37.
                        let audio = if warp_recorded {
                            auto_warp::warp_recorded(audio, &tempo, start, end, warp_mode)
                        } else {
                            audio
                        };
                        let audio = if edge_fades {
                            audio.with_edge_fades()
                        } else {
                            audio
                        };
                        let mut clip =
                            Clip::new(id, start, end.saturating_sub(start), ClipData::Audio(audio));
                        clip.name = file_stem(path);
                        clip.colour = look.new_clip_colour(id);
                        // Spec: comping R10. The take goes on a take lane too,
                        // and the main lane's clip is a copy of it.
                        crate::recording::put_on_lane(editor, *track, clip.clone())?;
                        clip.id = editor.new_id();
                        clip.colour = look.new_clip_colour(clip.id);
                        crate::arrangement::placing::clear(editor, *track, start, end, &placed)?;
                        placed.insert(clip.id);
                        editor.apply(bl_model::Command::CreateClip {
                            track: *track,
                            clip,
                        })?;
                    }
                    crate::recording::place_midi(
                        editor,
                        &midi,
                        take,
                        rate,
                        overdub,
                        true,
                        &mut placed,
                    )?;
                    let made: Vec<_> = placed.iter().copied().collect();
                    crate::colours::colour_new(editor, look, &[], &made)?;
                    crate::session_capture::place(editor, &watch, &runs, &armed, end)?;
                    // Spec: audio-clips R26. A take that meets a clip
                    // already there is crossfaded with it.
                    crate::arrangement::fades::settle(editor, &before, edge_fades)
                })
            }
            Some(_) => {
                let frames = files.first().map_or(0, |file| file.frames);
                self.place_loop_passes(frames, &imported, &midi, take, rate, delay)
            }
        };
        if let Err(error) = applied {
            self.fail_recording(format!("Recording failed: {error}"));
            return;
        }
        // Spec: comping R13. A recording that adds a take lane to a track
        // shows the track's take lanes.
        let grown: Vec<TrackId> = self
            .editor
            .document()
            .tracks()
            .filter(|track| {
                take_lanes_before
                    .iter()
                    .any(|(id, was)| *id == track.id() && track.take_lanes().len() > *was)
            })
            .map(bl_model::Track::id)
            .collect();
        for track in grown {
            self.arrangement.keep_take_lanes_shown(track);
        }
        if let Some(report) = &mut self.last_recording {
            report.clips_made = true;
        }
        for (_, hash, path, size) in imported {
            self.store.add_media(hash, &path, size);
        }
        self.dirty = true;
        self.document_changed();
        self.touch();
        if dropped > 0 {
            self.set_message(
                format!(
                    "Recording ended early: {dropped} frames were dropped because the disk \
                     couldn't keep up"
                ),
                true,
            );
        }
    }

    /// Shows why a finished recording made no clips, in the Status Bar and
    /// in the last recording's report.
    pub(super) fn fail_recording(&mut self, message: impl Into<String>) {
        let message = message.into();
        if let Some(report) = &mut self.last_recording {
            report.error = Some(message.clone());
        }
        self.set_message(message, true);
    }

    /// The recording under way, for `transport.state`: its armed tracks and
    /// the frames dropped so far. `None` when no recording is under way.
    #[must_use]
    pub fn running_recording(&self) -> Option<RunningRecording> {
        self.recording.as_ref().map(|recording| RunningRecording {
            armed_tracks: recording.armed().to_vec(),
            dropped_frames: recording.dropped(),
            session_slots: self.session_slots(),
        })
    }

    /// What the last recording that ended came to, for `transport.state`.
    #[must_use]
    pub fn last_recording(&self) -> Option<&RecordingReport> {
        self.last_recording.as_ref()
    }

    /// Puts a take that went round the loop on its tracks (the recording
    /// spec, R12 and R16): the lead-in before the loop's start is a clip on
    /// the main lane, and each pass is an undo step that puts the pass on a
    /// take lane and copies it to the main lane over what was there, so Undo
    /// run again and again unrolls the passes from the last. Every audio clip
    /// plays its pass of the one recorded file, which holds all of them.
    /// `frames` is how many frames each track's file holds: the rings fill
    /// together. A MIDI track's passes are cut from its messages by the
    /// frames the take had run for, and go the same way, the main lane
    /// merging each one with MIDI Arrangement Overdub on. A negative `delay`
    /// places every pass that much later, as it does a plain take's clip.
    fn place_loop_passes(
        &mut self,
        frames: u64,
        imported: &[(TrackId, ContentHash, PathBuf, u64)],
        midi: &[crate::recording::MidiTake],
        take: bl_engine::TakeBounds,
        rate: SampleRate,
        delay: i64,
    ) -> Result<(), bl_model::EditError> {
        let tempo = self.editor.document().settings().tempo.clone();
        let edge_fades = self.create_fades_on_clip_edges();
        let warp_recorded = self.warp_recorded_audio();
        let warp_mode = auto_warp::warp_mode(self.prefs.warp.default_warp_mode);
        let overdub = self.editor.document().settings().midi_overdub;
        let look = self.prefs.look.clone();
        let look = &look;
        let (lead_in, passes) = crate::recording::passes(take, frames);
        let (midi_lead_in, midi_passes) = crate::recording::passes(take, take.frames);
        let later = Frames((-delay).max(0));
        let seconds = |frames: u64| frames as f64 / rate.as_f64();
        let ticks = |frame: Frames| tempo.frames_to_ticks(frame, rate);
        let audio = |hash: ContentHash, pass: &crate::recording::Pass| {
            let (from, to) = (
                seconds(pass.file_from),
                seconds(pass.file_from + pass.frames),
            );
            let audio = AudioData {
                start: from,
                end: Some(to),
                loop_start: from,
                loop_end: Some(to),
                ..AudioData::new(hash)
            };
            // Spec: warping R37.
            let audio = if warp_recorded {
                let start = ticks(pass.song_start + later);
                let end = ticks(pass.song_start + later + Frames(pass.frames as i64));
                auto_warp::warp_recorded(audio, &tempo, start, end, warp_mode)
            } else {
                audio
            };
            if edge_fades {
                audio.with_edge_fades()
            } else {
                audio
            }
        };
        // The pass's clip, named after its file.
        let clip_of =
            |editor: &mut Editor, name: &str, pass: &crate::recording::Pass, data: AudioData| {
                let start = ticks(pass.song_start + later);
                let end = ticks(pass.song_start + later + Frames(pass.frames as i64));
                let mut clip = Clip::new(
                    editor.new_id(),
                    start,
                    end.saturating_sub(start),
                    ClipData::Audio(data),
                );
                clip.name = name.to_owned();
                clip.colour = look.new_clip_colour(clip.id);
                clip
            };
        let put_on_main =
            |editor: &mut Editor, track: TrackId, clip: Clip| -> Result<(), bl_model::EditError> {
                crate::arrangement::placing::clear(
                    editor,
                    track,
                    clip.start,
                    clip.end(),
                    &std::collections::BTreeSet::new(),
                )?;
                editor.apply(bl_model::Command::CreateClip { track, clip })?;
                Ok(())
            };
        let ends = |pass: &crate::recording::Pass| bl_engine::TakeBounds {
            start: pass.song_start,
            end: pass.song_start + Frames(pass.frames as i64),
            wrap: None,
            frames: pass.frames,
        };
        for step in 0..passes.len().max(midi_passes.len()).max(1) {
            self.editor
                .transact(crate::recording::LOOP_UNDO_LABEL, |editor| {
                    for (track, hash, path, _) in imported {
                        let name = file_stem(path);
                        if let (0, Some(pass)) = (step, &lead_in) {
                            let clip = clip_of(editor, &name, pass, audio(*hash, pass));
                            put_on_main(editor, *track, clip)?;
                        }
                        let Some(pass) = passes.get(step) else {
                            continue;
                        };
                        // Spec: comping R10. The pass goes on a take lane and
                        // is copied to the main lane.
                        let clip = clip_of(editor, &name, pass, audio(*hash, pass));
                        crate::recording::put_on_lane(editor, *track, clip.clone())?;
                        let id = editor.new_id();
                        let copy = Clip {
                            id,
                            colour: look.new_clip_colour(id),
                            ..clip
                        };
                        put_on_main(editor, *track, copy)?;
                    }
                    let mut placed = std::collections::BTreeSet::new();
                    if let (0, Some(pass)) = (step, &midi_lead_in) {
                        let cut = crate::recording::midi_in_pass(midi, take, pass);
                        crate::recording::place_midi(
                            editor,
                            &cut,
                            ends(pass),
                            rate,
                            overdub,
                            false,
                            &mut placed,
                        )?;
                    }
                    if let Some(pass) = midi_passes.get(step) {
                        let cut = crate::recording::midi_in_pass(midi, take, pass);
                        crate::recording::place_midi(
                            editor,
                            &cut,
                            ends(pass),
                            rate,
                            overdub,
                            true,
                            &mut placed,
                        )?;
                    }
                    let made: Vec<_> = placed.iter().copied().collect();
                    crate::colours::colour_new(editor, look, &[], &made)?;
                    Ok::<(), bl_model::EditError>(())
                })?;
        }
        Ok(())
    }

    /// Split Clip (note-editing R41): `arrangement.split` on the clip the
    /// Clip View shows at the editor's time selection, then the part
    /// inside the selection is selected.
    pub(super) fn split_shown_clip(&mut self) -> Result<Value, ShellError> {
        let id = note_ids::SPLIT_CLIP;
        let (args, clip, (from, to)) =
            clip_view::split_clip_plan(self.editor.document(), &self.clip_view, &self.arrangement)
                .map_err(|failure| edit_error(id, failure))?;
        let track = self.editor.document().clip_track(clip);
        let mut result = self.run_command(edit_ids::SPLIT, args)?;
        let doc = self.editor.document();
        let inside: Vec<_> = track
            .and_then(|track| doc.track(track))
            .map(|track| {
                track
                    .clips()
                    .filter(|part| part.start >= from && part.end() <= to)
                    .map(|part| part.id)
                    .collect()
            })
            .unwrap_or_default();
        if !inside.is_empty() {
            if let Some(fields) = result.as_object_mut() {
                fields.insert(
                    "selected_clips".to_owned(),
                    serde_json::to_value(&inside).unwrap_or(Value::Null),
                );
            }
            self.arrangement.select_clips(doc, inside);
        }
        self.touch();
        Ok(result)
    }
}

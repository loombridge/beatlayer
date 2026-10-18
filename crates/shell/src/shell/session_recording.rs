//! Recording into the Session View's slots (WP-3.11).
//!
//! A Session recording uses the Arrangement's machinery: rings for the armed
//! tracks, a writer for audio files, and a take the engine publishes. What
//! differs is where the take goes. The engine starts it on the launch grid
//! and ends it on the grid ([`SessionCommand::Record`]), and here the clips
//! go into slots: a new MIDI clip exists in its slot from the take's start
//! and grows with it, a new audio clip appears once its file is finished,
//! and an overdub adds the notes to the clip that plays, one loop pass at a
//! time.
//!
//! The document's transaction stays open from the command to the end of the
//! recording, so a new recording is one undo step. `shortcut:` edits made
//! while it is open join that step and Undo waits until it ends; upgrade by
//! applying the growth outside the history.

use bl_core::time::{Frames, Ticks};
use bl_engine::{SessionCommand, SessionTarget, SlotState, SlotStatus};
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, ClipId, Command, Document, EnvelopeKind, MidiData,
    SceneId, TrackId,
};
use bl_shell_spec::session_record::{
    self as record_ids, RecordingSlot, RecordingStarted, SlotArgs,
};

use super::*;

/// The undo step a slot's own record button makes, as Live names it
/// (observed 17 October 2026).
const SLOT_RECORDING_LABEL: &str = "Slot Recording";

/// The shortest a growing clip is: a sixteenth note.
const MIN_LENGTH: Ticks = Ticks::SIXTEENTH;

/// Where one armed track records, before the slot exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pick {
    pub(super) track: TrackId,
    /// The scene whose slot it is; `None` for a scene inserted at the end.
    pub(super) scene: Option<SceneId>,
    pub(super) overdub: bool,
}

/// One slot a recording goes into.
pub(super) struct RunSlot {
    slot: RecordingSlot,
    kind: TrackKind,
    /// The song frame an overdubbed clip launched on.
    started: Option<Frames>,
    /// The clip a new MIDI recording grows, once the take has begun.
    clip: Option<ClipId>,
    /// How many messages and how long the clip last showed.
    shown: (usize, Ticks),
}

/// The Session recording under way.
pub(super) struct SessionRun {
    slots: Vec<RunSlot>,
}

/// Whether the engine is playing a slot's clip.
pub(super) fn playing(
    states: &[SlotStatus],
    track: TrackId,
    scene: SceneId,
) -> Option<&SlotStatus> {
    states.iter().find(|status| {
        status.track == track
            && status.scene == scene
            && matches!(status.state, SlotState::Playing | SlotState::QueuedToStop)
    })
}

/// Where Session Record puts each armed track's recording (the session
/// recording spec, R1): a MIDI track whose clip plays overdubs it; any other
/// track records a new clip into the scene `from`'s slot if it is empty, else
/// into the next empty slot below, else into a scene inserted at the end.
fn picks(doc: &Document, from: Option<SceneId>, states: &[SlotStatus]) -> Vec<Pick> {
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            doc.track(id).is_some_and(|track| {
                track.mixer().arm && matches!(track.kind(), TrackKind::Midi | TrackKind::Audio)
            })
        })
        .filter_map(|id| pick(doc, id, from, states))
        .collect()
}

/// Where one track's recording goes (`picks`' rule, for one track), which
/// Capture MIDI uses too.
pub(super) fn pick(
    doc: &Document,
    id: TrackId,
    from: Option<SceneId>,
    states: &[SlotStatus],
) -> Option<Pick> {
    let scenes: Vec<SceneId> = doc.scenes().map(|scene| scene.id).collect();
    let first = from
        .and_then(|from| scenes.iter().position(|&scene| scene == from))
        .unwrap_or(0);
    let track = doc.track(id)?;
    if track.kind() == TrackKind::Midi
        && let Some(status) = scenes.iter().find_map(|&scene| {
            let clip = track.slot(scene)?;
            matches!(clip.data, ClipData::Midi(_))
                .then(|| playing(states, id, scene))
                .flatten()
        })
    {
        return Some(Pick {
            track: id,
            scene: Some(status.scene),
            overdub: true,
        });
    }
    let empty = scenes[first..]
        .iter()
        .copied()
        .find(|&scene| track.slot(scene).is_none());
    Some(Pick {
        track: id,
        scene: empty,
        overdub: false,
    })
}

impl Shell {
    /// Runs one of the Session recording commands (WP-3.11).
    pub(super) fn run_session_record(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        match id {
            record_ids::RECORD => {
                Self::args::<NoArgs>(id, args)?;
                if self.session_run.is_some() {
                    // Spec: session-recording R4. Pressing the button again
                    // ends the recording and the clips loop.
                    self.audio
                        .send_session(SessionCommand::StopRecording)
                        .map_err(unavailable)?;
                    return Ok(to_value(&RecordingStarted::default()));
                }
                let from = self.session_scene();
                let picked = picks(self.editor.document(), from, &self.session_states);
                self.begin_session_recording(picked, None)
                    .map(|started| to_value(&started))
                    .map_err(unavailable)
            }
            record_ids::RECORD_SLOT => {
                let SlotArgs { track, scene } = Self::args(id, args)?;
                let doc = self.editor.document();
                let armed = doc.track(track).is_some_and(|t| t.mixer().arm);
                if !armed {
                    return Err(unavailable("arm the track first".to_owned()));
                }
                if doc.slot(track, scene).is_some() {
                    return Err(unavailable(
                        "the slot holds a clip: its button launches it, and `session.overdub` \
                         records into it"
                            .to_owned(),
                    ));
                }
                let pick = Pick {
                    track,
                    scene: Some(scene),
                    overdub: false,
                };
                self.begin_session_recording(vec![pick], Some(SLOT_RECORDING_LABEL))
                    .map(|started| to_value(&started))
                    .map_err(unavailable)
            }
            record_ids::OVERDUB => {
                let SlotArgs { track, scene } = Self::args(id, args)?;
                let doc = self.editor.document();
                let midi_clip = doc
                    .slot(track, scene)
                    .is_some_and(|clip| matches!(clip.data, ClipData::Midi(_)));
                if !doc.track(track).is_some_and(|t| t.mixer().arm) || !midi_clip {
                    return Err(unavailable(
                        "overdubbing needs an armed MIDI track and a MIDI clip in the slot"
                            .to_owned(),
                    ));
                }
                if playing(&self.session_states, track, scene).is_none() {
                    return Err(unavailable("launch the clip first".to_owned()));
                }
                let pick = Pick {
                    track,
                    scene: Some(scene),
                    overdub: true,
                };
                self.begin_session_recording(vec![pick], None)
                    .map(|started| to_value(&started))
                    .map_err(unavailable)
            }
            record_ids::STOP_RECORDING => {
                Self::args::<NoArgs>(id, args)?;
                if self.session_run.is_some() {
                    self.audio
                        .send_session(SessionCommand::StopRecording)
                        .map_err(unavailable)?;
                }
                Ok(Value::Null)
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The scene Session Record starts from: the selected scene, else the
    /// selected slot's, else the first.
    pub(super) fn session_scene(&self) -> Option<SceneId> {
        let state = self.session.state();
        state
            .scenes
            .first()
            .copied()
            .or_else(|| state.slots.first().map(|slot| slot.scene))
    }

    /// Start Recording on Scene Launch (C-SCENE-012): launching a scene
    /// also records into the empty slots of armed tracks in it. The caller
    /// has sent the launch, so both start on the same grid line.
    pub(super) fn record_on_scene_launch(&mut self, scene: SceneId) {
        if !self.prefs.launch.start_recording_on_scene_launch || self.recording.is_some() {
            return;
        }
        let doc = self.editor.document();
        let picked: Vec<Pick> = doc
            .track_order()
            .into_iter()
            .filter(|&id| {
                doc.track(id).is_some_and(|t| {
                    t.mixer().arm
                        && matches!(t.kind(), TrackKind::Midi | TrackKind::Audio)
                        && t.slot(scene).is_none()
                })
            })
            .map(|track| Pick {
                track,
                scene: Some(scene),
                overdub: false,
            })
            .collect();
        if !picked.is_empty()
            && let Err(reason) = self.begin_session_recording(picked, None)
        {
            self.set_message(reason, true);
        }
    }

    /// Starts a Session recording into the slots `picked` names: opens the
    /// undo step, inserts the scene a track needs, gives the engine its
    /// rings, and tells it to start on the grid.
    fn begin_session_recording(
        &mut self,
        picked: Vec<Pick>,
        label: Option<&'static str>,
    ) -> Result<RecordingStarted, String> {
        if self.recording.is_some() {
            return Err("a recording is already under way; end it first".to_owned());
        }
        if picked.is_empty() {
            return Err("arm an audio or MIDI track to record onto".to_owned());
        }
        // Live's names for the steps (observed 17 October 2026): "Overdub
        // Record" when Session Record overdubs, "Slot Recording" for a
        // slot's own button.
        let label = label.unwrap_or(if picked.iter().all(|pick| pick.overdub) {
            crate::automation_record::OVERDUB_UNDO_LABEL
        } else {
            crate::recording::UNDO_LABEL
        });
        self.editor.begin(label);
        match self.start_session_recording(picked) {
            Ok(started) => Ok(started),
            Err(reason) => {
                let _ = self.editor.cancel();
                self.document_changed();
                self.touch();
                Err(reason)
            }
        }
    }

    fn start_session_recording(&mut self, picked: Vec<Pick>) -> Result<RecordingStarted, String> {
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
        // A scene inserted at the end serves every track that needs one.
        let mut inserted = None;
        let mut audio = Vec::new();
        let mut midi = Vec::new();
        let mut slots = Vec::new();
        let mut skipped_audio = false;
        for pick in picked {
            let scene = match pick.scene {
                Some(scene) => scene,
                None => match inserted {
                    Some(scene) => scene,
                    None => {
                        let scene: SceneId = self.editor.new_id();
                        let index = self.editor.document().scenes().len();
                        self.editor
                            .apply(Command::CreateScene { id: scene, index })
                            .map_err(|e| e.to_string())?;
                        inserted = Some(scene);
                        scene
                    }
                },
            };
            let document = self.editor.document();
            let Some(track) = document.track(pick.track) else {
                continue;
            };
            let kind = track.kind();
            match (kind, &track.routing().input) {
                (TrackKind::Midi, _) => midi.push(pick.track),
                (TrackKind::Audio, Input::External { channel }) if folder.is_some() => {
                    audio.push(crate::recording::Track {
                        track: pick.track,
                        channel: *channel,
                        name: crate::recording::file_name(
                            &crate::routing::track_label(document, pick.track),
                            take,
                            &stamp,
                        ),
                    });
                }
                (TrackKind::Audio, Input::External { .. }) => {
                    skipped_audio = true;
                    continue;
                }
                _ => continue,
            }
            let started = playing(&self.session_states, pick.track, scene).and_then(|s| s.started);
            slots.push(RunSlot {
                slot: RecordingSlot {
                    track: pick.track,
                    scene,
                    overdub: pick.overdub,
                },
                kind,
                started,
                clip: None,
                shown: (0, Ticks::ZERO),
            });
        }
        if skipped_audio {
            self.set_message(
                "the set has never been saved; use `set.save_as` to give it a project folder, \
                 where its recordings go",
                true,
            );
        }
        if slots.is_empty() {
            return Err(
                "none of the armed tracks can record: an audio track records from \
                        Ext. In, in a saved set"
                    .to_owned(),
            );
        }
        let targets = slots
            .iter()
            .map(|run| SessionTarget {
                track: run.slot.track,
                scene: run.slot.scene,
                overdub: run.slot.overdub,
            })
            .collect();
        let (mut plan, mut recording) =
            crate::recording::start(folder.as_deref(), rate, &audio, &midi, targets)?;
        let delay = self
            .audio
            .recording_delay()
            .ok_or("no audio device is open")?;
        plan.delay = delay;
        recording.delay = delay;
        self.audio.record(plan)?;
        self.audio.send_session(SessionCommand::Record)?;
        let started = RecordingStarted {
            slots: slots.iter().map(|run| run.slot).collect(),
        };
        self.recording = Some(recording);
        self.session_run = Some(SessionRun { slots });
        self.session_automation = crate::automation_record::SessionTake::default();
        self.session_take_start = None;
        self.document_changed();
        self.touch();
        Ok(started)
    }

    /// The slots a Session recording under way goes into, for
    /// `transport.state`.
    pub(super) fn session_slots(&self) -> Vec<RecordingSlot> {
        self.session_run
            .as_ref()
            .map(|run| run.slots.iter().map(|slot| slot.slot).collect())
            .unwrap_or_default()
    }

    /// Grows the MIDI clips of new recordings to the take so far: the clip
    /// is made when the take begins, and its length and notes follow each
    /// poll (the session recording spec, R7).
    pub(super) fn grow_session_clips(&mut self, engine: &EngineTransport) {
        if let Some(so_far) = engine.take_so_far {
            self.session_take_start = Some(so_far.start);
        }
        let (Some(run), Some(so_far)) = (self.session_run.as_mut(), engine.take_so_far) else {
            return;
        };
        let Some(rate) = self.audio.sample_rate().and_then(SampleRate::new) else {
            return;
        };
        let tempo = self.editor.document().settings().tempo.clone();
        let length = (tempo.frames_to_ticks(so_far.end, rate)
            - tempo.frames_to_ticks(so_far.start, rate))
        .max(MIN_LENGTH);
        let mut changed = false;
        let mut failure = None;
        for slot in run
            .slots
            .iter_mut()
            .filter(|slot| !slot.slot.overdub && slot.kind == TrackKind::Midi)
        {
            let messages = self
                .recording
                .as_ref()
                .map_or(&[][..], |recording| recording.midi_so_far(slot.slot.track));
            if slot.clip.is_some() && slot.shown == (messages.len(), length) {
                continue;
            }
            let notes =
                crate::recording::session_notes(messages, so_far.start, so_far.end, &tempo, rate);
            let data = ClipData::Midi(new_midi(notes));
            let result = match slot.clip {
                Some(clip) => self
                    .editor
                    .apply(Command::SetClipLength { clip, length })
                    .and_then(|_| self.editor.apply(Command::SetClipData { clip, data })),
                None => {
                    let clip: ClipId = self.editor.new_id();
                    slot.clip = Some(clip);
                    self.editor.apply(Command::CreateSlotClip {
                        track: slot.slot.track,
                        scene: slot.slot.scene,
                        clip: Clip::new(clip, Ticks::ZERO, length, data),
                    })
                }
            };
            match result {
                Ok(_) => {
                    slot.shown = (messages.len(), length);
                    changed = true;
                }
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            self.set_message(format!("Recording failed to grow its clip: {error}"), true);
        }
        if changed {
            self.dirty = true;
            self.document_changed();
            self.touch();
        }
    }

    /// Ends a Session recording that made nothing: puts the set back as it
    /// was before the recording began.
    pub(super) fn abandon_session_recording(&mut self) {
        if self.session_run.take().is_some() {
            self.session_automation = crate::automation_record::SessionTake::default();
            let _ = self.editor.cancel();
            self.document_changed();
            self.touch();
        }
    }

    /// Makes the clips of a finished Session recording: the open undo step
    /// gets the new clips, and each overdub pass is a step of its own (the
    /// session recording spec, R7 and R8).
    pub(super) fn finish_session_recording(
        &mut self,
        run: SessionRun,
        outcome: crate::recording::Outcome,
    ) {
        let crate::recording::Outcome {
            take,
            delay,
            files,
            midi,
            dropped,
            launches: _,
        } = outcome;
        let files = files.unwrap_or_default();
        let (Some(take), Some(rate)) = (take, self.audio.sample_rate().and_then(SampleRate::new))
        else {
            // The take never began: Stop came before the grid line.
            self.abandon_session_recording_with(run);
            return;
        };
        let tempo = self.editor.document().settings().tempo.clone();
        let ticks = |frames: Frames| tempo.frames_to_ticks(frames, rate);
        let length = (ticks(take.end) - ticks(take.start)).max(MIN_LENGTH);
        let edge_fades = self.create_fades_on_clip_edges();
        let mut imported = Vec::new();
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
                    self.abandon_session_recording_with(run);
                    self.fail_recording(format!(
                        "Recording failed: {}: {error}",
                        file.path.display()
                    ));
                    return;
                }
            }
        }
        let applied = (|| -> Result<(), bl_model::EditError> {
            for slot in run.slots.iter().filter(|slot| !slot.slot.overdub) {
                let RecordingSlot { track, scene, .. } = slot.slot;
                match slot.kind {
                    TrackKind::Midi => {
                        let messages = midi
                            .iter()
                            .find(|played| played.track == track)
                            .map_or(&[][..], |played| &played.messages);
                        let notes = crate::recording::session_notes(
                            messages, take.start, take.end, &tempo, rate,
                        );
                        let data = ClipData::Midi(new_midi(notes));
                        let clip = match slot.clip {
                            Some(clip) => {
                                self.editor.apply(Command::SetClipLength { clip, length })?;
                                self.editor.apply(Command::SetClipData { clip, data })?;
                                clip
                            }
                            None => {
                                let clip: ClipId = self.editor.new_id();
                                self.editor.apply(Command::CreateSlotClip {
                                    track,
                                    scene,
                                    clip: Clip::new(clip, Ticks::ZERO, length, data),
                                })?;
                                clip
                            }
                        };
                        // Spec: midi-controller-data R14. The controllers
                        // become the clip's envelopes when the take ends.
                        let moves =
                            crate::recording::session_moves(messages, take.start, &tempo, rate);
                        crate::recording::write_moves(&mut self.editor, clip, &moves, None)?;
                    }
                    _ => {
                        let Some((_, hash, path, _)) =
                            imported.iter().find(|(file, ..)| *file == track)
                        else {
                            continue;
                        };
                        // Spec: recording R7. The audio is moved `delay`
                        // frames earlier.
                        let start = ticks(take.start + Frames((-delay).max(0)));
                        let end = ticks(take.end - Frames(delay));
                        let audio = AudioData {
                            looping: true,
                            ..AudioData::new(*hash)
                        };
                        let audio = if edge_fades {
                            audio.with_edge_fades()
                        } else {
                            audio
                        };
                        let mut clip = Clip::new(
                            self.editor.new_id(),
                            Ticks::ZERO,
                            end.saturating_sub(start).max(MIN_LENGTH),
                            ClipData::Audio(audio),
                        );
                        clip.name = file_stem(path);
                        self.editor
                            .apply(Command::CreateSlotClip { track, scene, clip })?;
                    }
                }
            }
            // Spec: session-automation R3, R11. The moves made while the take ran
            // go into the clips, in the recording's undo step.
            self.session_automation.commit(
                &mut self.editor,
                crate::recording::UNDO_LABEL,
                |clip, new_clip| {
                    if new_clip {
                        clip.length
                    } else {
                        bl_engine::session_pass_end(clip).unwrap_or(clip.length)
                    }
                },
            )?;
            Ok(())
        })();
        self.session_automation = crate::automation_record::SessionTake::default();
        let committed = match applied {
            Ok(()) => self.editor.commit().map(|_| ()),
            Err(error) => {
                let _ = self.editor.cancel();
                Err(error)
            }
        };
        if let Err(error) = committed {
            self.document_changed();
            self.touch();
            self.fail_recording(format!("Recording failed: {error}"));
            return;
        }
        // Spec: session-recording R8. Each loop pass of an overdub is an
        // undo step of its own, so Undo run again and again unrolls them.
        let mut failure = None;
        for slot in run.slots.iter().filter(|slot| slot.slot.overdub) {
            let (Some(started), Some(played)) = (
                slot.started,
                midi.iter().find(|played| played.track == slot.slot.track),
            ) else {
                continue;
            };
            let RecordingSlot { track, scene, .. } = slot.slot;
            let Some(clip) = self.editor.document().slot(track, scene).cloned() else {
                continue;
            };
            let ClipData::Midi(data) = &clip.data else {
                continue;
            };
            let notes =
                crate::recording::session_notes(&played.messages, started, take.end, &tempo, rate);
            let moves = crate::recording::session_moves(&played.messages, started, &tempo, rate);
            let mut passes = crate::recording::overdub_passes(data, clip.length, notes);
            let mut stepped = crate::recording::overdub_moves(data, clip.length, &moves);
            // A pass with notes only, or with controllers only, is a step too.
            let numbers: std::collections::BTreeSet<usize> =
                passes.keys().chain(stepped.keys()).copied().collect();
            for number in numbers {
                let pass = passes.remove(&number).unwrap_or_default();
                let (end, steps) = stepped.remove(&number).unwrap_or_default();
                let result = self
                    .editor
                    .transact(crate::recording::LOOP_UNDO_LABEL, |editor| {
                        let Some(ClipData::Midi(mut data)) =
                            editor.document().clip(clip.id).map(|c| c.data.clone())
                        else {
                            return Ok(());
                        };
                        data.notes.extend(pass);
                        editor.apply(Command::SetClipData {
                            clip: clip.id,
                            data: ClipData::Midi(data),
                        })?;
                        // Spec: midi-controller-data R14. A controller
                        // replaces its envelope from its first message to the
                        // end of the loop.
                        crate::recording::write_moves(editor, clip.id, &steps, Some(end))
                    });
                if let Err(error) = result {
                    failure = Some(error.to_string());
                }
            }
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
        if let Some(error) = failure {
            self.fail_recording(format!("Recording failed: {error}"));
        } else if dropped > 0 {
            self.set_message(
                format!(
                    "Recording ended early: {dropped} frames were dropped because the disk \
                     couldn't keep up"
                ),
                true,
            );
        }
    }

    fn abandon_session_recording_with(&mut self, run: SessionRun) {
        self.session_run = Some(run);
        self.abandon_session_recording();
    }
}

impl Shell {
    /// The slot the control moves of `track` go into while a Session
    /// recording runs, and where its clip is `now`: the slot the recording
    /// makes a clip in, or the MIDI clip playing on the track (that of an
    /// armed track overdubbed, or, with Record Session automation in at All
    /// Tracks, of any track). `None` for a track nothing is recorded for.
    /// The flag is whether the clip is one the recording makes.
    fn session_move_slot(&self, track: TrackId) -> Option<(SceneId, bool, Ticks)> {
        let rate = self.audio.sample_rate().and_then(SampleRate::new)?;
        let document = self.editor.document();
        let tempo = &document.settings().tempo;
        let now = self.arrangement.transport().position;
        let run = self.session_run.as_ref()?;
        let armed = run.slots.iter().find(|slot| slot.slot.track == track);
        let scene = match armed {
            Some(slot) => slot.slot.scene,
            None if self.prefs.record_session_automation_in
                == crate::settings::SessionAutomationTracks::AllTracks =>
            {
                self.session_states
                    .iter()
                    .find(|status| {
                        status.track == track
                            && matches!(status.state, SlotState::Playing | SlotState::QueuedToStop)
                    })?
                    .scene
            }
            None => return None,
        };
        if armed.is_some_and(|slot| !slot.slot.overdub) {
            // A clip the recording makes: its time is the take's.
            if armed?.kind != TrackKind::Midi {
                return None;
            }
            let start = tempo.frames_to_ticks(self.session_take_start?, rate);
            let at = now - start;
            return (at >= Ticks::ZERO).then_some((scene, true, at));
        }
        let clip = document.slot(track, scene)?;
        // The engine reports an overdubbed slot as recording, so the frame
        // its clip launched on is the one the run kept.
        let started = match armed {
            Some(slot) => slot.started?,
            None => playing(&self.session_states, track, scene)?.started?,
        };
        let since = now - tempo.frames_to_ticks(started, rate);
        let at = bl_engine::session_content_tick(clip, true, since.max(Ticks::ZERO))?;
        Some((scene, false, at))
    }

    /// Notes a control move for the Session recording (the Session
    /// automation spec, R1): with Automation Arm on, a move of a control of a
    /// track whose clips the recording writes lands in the clip at its place
    /// in the clip's own time.
    pub(super) fn note_session_move(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        before: f64,
        value: f64,
    ) {
        if let Some((scene, new_clip, at)) = self.session_move_slot(track) {
            self.session_automation
                .write((track, scene, target), new_clip, before, at, value);
        }
    }

    /// Notes that the controls being moved were let go (a drag ended).
    pub(super) fn release_session_moves(&mut self) {
        if self.session_automation.is_empty() {
            return;
        }
        let mut positions = std::collections::BTreeMap::new();
        for (track, scene, _) in self.session_automation.keys() {
            if let Some((found, _, at)) = self.session_move_slot(track)
                && found == scene
            {
                positions.insert((track, scene), at);
            }
        }
        self.session_automation
            .release(|(track, scene, _)| positions.get(&(*track, *scene)).copied());
    }

    /// Launching a clip gives its automation envelopes back to it: the
    /// controls a hand move took from them are theirs again from the launch
    /// (the Session automation spec, R5), one undo step when any had been
    /// taken.
    pub(super) fn re_enable_launched_automation(&mut self, command: SessionCommand) {
        let document = self.editor.document();
        let slots: Vec<(TrackId, SceneId)> = match command {
            SessionCommand::Launch { track, scene } => vec![(track, scene)],
            SessionCommand::LaunchScene(scene) => document
                .track_order()
                .into_iter()
                .map(|track| (track, scene))
                .collect(),
            _ => return,
        };
        let mut targets = Vec::new();
        for (track, scene) in slots {
            let Some(clip) = document.slot(track, scene) else {
                continue;
            };
            targets.extend(
                clip.envelopes()
                    .filter(|e| e.kind == EnvelopeKind::Automation && !e.enabled)
                    .map(|e| (track, e.target)),
            );
        }
        if targets.is_empty() {
            return;
        }
        let result = self.editor.transact("Re-Enable Automation", |editor| {
            for (track, target) in targets {
                editor.apply(Command::ReEnableAutomationFor { track, target })?;
            }
            Ok::<(), bl_model::EditError>(())
        });
        if result.is_ok() {
            self.dirty = true;
            self.document_changed();
            self.touch();
        }
    }
}

/// A new recording's MIDI data: the notes in a clip that loops.
fn new_midi(notes: Vec<bl_model::Note>) -> MidiData {
    MidiData {
        looping: true,
        ..MidiData::new(notes)
    }
}

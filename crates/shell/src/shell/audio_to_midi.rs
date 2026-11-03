//! Convert Harmony, Melody and Drums to New MIDI Track through the shell
//! (IMP-06; the audio-to-midi spec, R1 to R12, R19, R25 to R27): the
//! `audio_to_midi.*` commands, the progress window's state, and the edit
//! that lands once the notes are found. The transcription itself is in
//! [`crate::audio_to_midi`]; the track's instrument in
//! [`crate::audio_to_midi_racks`].

use bl_analysis::transcribe::Note as Found;
use bl_core::time::{TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, MidiData, Note, SceneId, TrackId, TrackKind,
};
use bl_shell_spec::audio_to_midi::{
    self as ids, AudioToMidiStatus, ConvertArgs, ConvertDialogStatus, ConvertDialogStep,
    ConvertKind,
};
use bl_shell_spec::stems::{JobState, JobStatus};

use super::*;
use crate::audio_to_midi::{self as work, Backend, FEATURE};
use crate::packages::Package;
use crate::stems::{Ended, Worker};

/// A confident onset within this many seconds of a sixteenth-note line moves
/// onto it, and is confident from this far (R15).
const SNAP_WITHIN: f64 = 0.020;
const SNAP_CONFIDENCE: f32 = 0.8;

/// A drum note is a sixteenth note long (R22).
const SIXTEENTH: i64 = TICKS_PER_QUARTER / 4;

/// Audio to MIDI's state in the shell.
#[derive(Default)]
pub(super) struct AudioToMidiState {
    /// The conversion, while it runs.
    running: Option<Running>,
    /// How the last conversion went.
    outcome: JobStatus,
    /// What the running or last conversion converts.
    last: Option<(ConvertKind, ClipId)>,
    /// The track, clip and note count the last conversion made.
    created: Option<(TrackId, ClipId, usize)>,
    /// The window, while it's open.
    dialog: Option<ConvertDialogStatus>,
    /// The conversion that waits for the packages to be installed.
    waiting: Option<(ConvertKind, ClipId)>,
    /// Notes that stand in for the transcription, for tests.
    fixture: Option<Vec<Found>>,
}

/// A conversion under way.
struct Running {
    worker: Worker<Vec<Found>>,
    plan: Plan,
}

/// What a conversion was started on, as the set held it then.
struct Plan {
    kind: ConvertKind,
    clip: Clip,
    audio: AudioData,
    tempo: TempoMap,
    /// Where the converted region starts and ends in the file, in seconds.
    region: (f64, f64),
}

impl Plan {
    /// Where the file's `seconds` plays, in ticks from the clip's Start
    /// marker (R7): through the warp markers of a warped clip, at the
    /// set's tempo for an unwarped one.
    fn offset(&self, seconds: f64) -> Ticks {
        let audio = &self.audio;
        if audio.warp {
            Ticks((audio.in_clip_form(seconds) - audio.start).round() as i64)
        } else {
            let base = self.tempo.ticks_to_seconds(self.clip.start);
            self.tempo.seconds_to_ticks(base + (seconds - audio.start)) - self.clip.start
        }
    }

    /// A position in the clip's own form (R6), in ticks from the Start
    /// marker.
    fn offset_of_form(&self, value: f64) -> Ticks {
        self.offset(self.audio.in_file_seconds(value))
    }

    /// Where the region (R4) starts, in ticks from the Start marker: before
    /// it when the loop brace starts earlier. The new clip's contents start
    /// here, so its Start marker sits this far into them.
    fn origin(&self) -> Ticks {
        self.offset(self.region.0).min(Ticks::ZERO)
    }

    /// The notes the transcription found, placed in the new clip's contents
    /// from [`Plan::origin`] (R7, R15, R22).
    fn place(&self, found: &[Found]) -> Vec<Note> {
        let origin = self.origin();
        let mut placed: Vec<(Note, f32)> = found
            .iter()
            .filter_map(|note| {
                let start = self.offset(self.region.0 + note.start);
                let end = self.offset(self.region.0 + note.end);
                (start >= origin).then(|| {
                    let start = self.snapped(start, note.confidence, origin);
                    (
                        Note::new(
                            note.pitch,
                            start - origin,
                            (end - start).max(Ticks(1)),
                            note.velocity,
                        ),
                        note.confidence,
                    )
                })
            })
            .collect();
        placed.sort_by_key(|(note, _)| (note.start, note.key));
        let drums = self.kind == ConvertKind::Drums;
        // A pitch's notes follow one another, and a drum hit is a sixteenth
        // note at most.
        for i in 0..placed.len() {
            let next = placed[i + 1..]
                .iter()
                .find(|(other, _)| other.key == placed[i].0.key)
                .map(|(other, _)| other.start);
            let note = &mut placed[i].0;
            if drums {
                note.length = Ticks(SIXTEENTH);
            }
            if let Some(next) = next {
                note.length = note.length.min(next - note.start);
            }
        }
        placed
            .into_iter()
            .map(|(note, _)| note)
            .filter(|note| note.length > Ticks::ZERO)
            .collect()
    }

    /// `start` moved onto the clip's sixteenth-note grid when the onset is
    /// confident and within 20 ms of a line (R15). The grid is the set's.
    fn snapped(&self, start: Ticks, confidence: f32, origin: Ticks) -> Ticks {
        if confidence < SNAP_CONFIDENCE {
            return start;
        }
        let at = self.clip.start + start;
        let line = Ticks((at.get() as f64 / SIXTEENTH as f64).round() as i64 * SIXTEENTH);
        let seconds = self.tempo.ticks_to_seconds(at);
        let within = self.tempo.seconds_to_ticks(seconds + SNAP_WITHIN) - at;
        if (line - at).get().abs() <= within.get() && line >= self.clip.start + origin {
            line - self.clip.start
        } else {
            start
        }
    }
}

impl Shell {
    /// The window of Convert … to New MIDI Track, if it's open.
    #[must_use]
    pub fn audio_to_midi_dialog(&self) -> Option<ConvertDialogStatus> {
        self.audio_to_midi.dialog.clone()
    }

    /// Lets these notes, in seconds from the region's start, stand in for
    /// the transcription, so a test converts without the packages.
    #[doc(hidden)]
    pub fn use_transcription_fixture(&mut self, notes: Vec<Found>) {
        self.audio_to_midi.fixture = Some(notes);
    }

    /// [`Shell::use_transcription_fixture`] for notes of `(pitch, start, end)`
    /// at velocity 100 and a doubtful onset, so a test in a crate without
    /// `bl-analysis` needs no more.
    #[doc(hidden)]
    pub fn use_transcription_notes(&mut self, notes: &[(u8, f64, f64)]) {
        self.use_transcription_fixture(
            notes
                .iter()
                .map(|&(pitch, start, end)| Found {
                    start,
                    end,
                    pitch,
                    velocity: 100,
                    confidence: 0.5,
                })
                .collect(),
        );
    }

    /// The packages Harmony and Melody need (R2).
    fn audio_to_midi_packages(&self) -> Vec<Package> {
        self.feature_packages(FEATURE)
    }

    /// Whether every package Harmony and Melody need is installed.
    #[must_use]
    pub fn audio_to_midi_packages_installed(&self) -> bool {
        let folder = self.packages_folder();
        let packages = self.audio_to_midi_packages();
        !packages.is_empty() && packages.iter().all(|p| p.installed(&folder))
    }

    /// Audio to MIDI's state, as `audio_to_midi.status` reads it.
    #[must_use]
    pub fn audio_to_midi_status(&self) -> AudioToMidiStatus {
        let state = &self.audio_to_midi;
        let packages = self.audio_to_midi_packages();
        let conversion = match &state.running {
            Some(running) => {
                let progress = running.worker.progress();
                JobStatus {
                    state: JobState::Running,
                    progress: progress.fraction(),
                    stage: progress.stage(),
                    error: None,
                }
            }
            None => state.outcome.clone(),
        };
        AudioToMidiStatus {
            installed: self.audio_to_midi_packages_installed(),
            folder: self.packages_folder().to_string_lossy().into_owned(),
            packages: packages.iter().map(|p| self.package_status(p)).collect(),
            install: self.packages_job(&packages),
            conversion,
            kind: state.last.map(|(kind, _)| kind),
            clip: state.last.map(|(_, clip)| clip),
            created_track: state.created.map(|(track, _, _)| track),
            created_clip: state.created.map(|(_, clip, _)| clip),
            notes: state.created.map(|(_, _, notes)| notes),
            dialog: state.dialog.clone(),
        }
    }

    /// The audio clip a conversion acts on (R1): the one named, or the one
    /// audio clip selected in either view.
    fn audio_to_midi_clip(&self, id: &str, clip: Option<ClipId>) -> Result<ClipId, ShellError> {
        // Exactly one clip: several selected clips are none, whatever the
        // Clip View shows.
        if clip.is_none() && self.arrangement.selected_clips().len() > 1 {
            return Err(unavailable(id, "select one audio clip first"));
        }
        self.slice_clip(&json!({ "clip": clip }))
            .map_err(|error| match error {
                ShellError::Unavailable { reason, .. } => unavailable(id, reason),
                ShellError::InvalidArgs { message, .. } => invalid(id, message),
                other => other,
            })
    }

    /// Whether an `audio_to_midi.*` command can run now: the menus' enabled
    /// state (R1).
    pub(super) fn can_audio_to_midi(&self, id: &str, args: &Value) -> bool {
        match id {
            ids::CONVERT => {
                let Ok(args) = serde_json::from_value::<ConvertArgs>(args.clone()) else {
                    return false;
                };
                self.audio_to_midi_clip(id, args.clip).is_ok()
            }
            _ => true,
        }
    }

    pub(super) fn run_audio_to_midi(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::STATUS => {}
            ids::CONVERT => {
                let args: ConvertArgs = Self::args(id, args)?;
                self.convert_to_midi(id, args)?;
            }
            ids::CANCEL => self.cancel_conversion(),
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.audio_to_midi_status()))
    }

    /// Stops the conversion and the download it waits for, and closes the
    /// window (R3).
    fn cancel_conversion(&mut self) {
        let packages = self.audio_to_midi_packages();
        if self.audio_to_midi.waiting.take().is_some() {
            self.cancel_packages(&packages);
        }
        if let Some(running) = &self.audio_to_midi.running {
            running.worker.progress().cancel();
        }
        self.audio_to_midi.dialog = None;
        self.touch();
    }

    /// `audio_to_midi.convert` (R2, R26): refuses what can't be converted,
    /// asks for the packages the first time, and starts the conversion.
    fn convert_to_midi(&mut self, id: &str, args: ConvertArgs) -> Result<(), ShellError> {
        let clip = self.audio_to_midi_clip(id, args.clip)?;
        let kind = args.kind;
        if self.audio_to_midi.running.is_some() {
            return Err(unavailable(id, "a conversion is already under way"));
        }
        if self.separating() {
            return Err(unavailable(id, "a stem separation is under way"));
        }
        let plan = self.conversion_plan(id, kind, clip)?;
        let fixture = self.audio_to_midi.fixture.is_some();
        if kind.needs_packages() && !fixture && !self.audio_to_midi_packages_installed() {
            // Spec: audio-to-midi R2. The package page opens instead, and
            // the conversion runs once the packages are installed.
            if self.audio_to_midi_packages().is_empty() {
                return Err(unavailable(
                    id,
                    "Harmony and Melody aren't available on this platform",
                ));
            }
            let step = if self.packages_active(&self.audio_to_midi_packages()) {
                ConvertDialogStep::Installing
            } else {
                ConvertDialogStep::Packages
            };
            self.audio_to_midi.waiting = Some((kind, clip));
            self.audio_to_midi.dialog = Some(ConvertDialogStatus { step, kind, clip });
            self.touch();
            return Ok(());
        }
        self.start_conversion(id, plan)
    }

    /// What a conversion of `clip` is started on (R4, R26).
    fn conversion_plan(
        &self,
        id: &str,
        kind: ConvertKind,
        clip: ClipId,
    ) -> Result<Plan, ShellError> {
        let doc = self.editor.document();
        let held = doc
            .clip(clip)
            .ok_or_else(|| invalid(id, "no clip has that ID"))?;
        let ClipData::Audio(audio) = &held.data else {
            return Err(unavailable(id, "only an audio clip can be converted"));
        };
        let file_seconds = self
            .file_seconds(audio)
            .ok_or_else(|| unavailable(id, "the clip's file isn't in the media store"))?;
        // Spec: audio-to-midi R4. The Start marker to the End marker, or
        // through the loop.
        let (from, to) = audio.crop_region(file_seconds);
        let region = (
            audio.in_file_seconds(from).clamp(0.0, file_seconds),
            audio.in_file_seconds(to).clamp(0.0, file_seconds),
        );
        if region.1 <= region.0 {
            return Err(unavailable(id, "the clip plays no audio to convert"));
        }
        Ok(Plan {
            kind,
            clip: held.clone(),
            audio: audio.clone(),
            tempo: doc.settings().tempo.clone(),
            region,
        })
    }

    fn start_conversion(&mut self, id: &str, plan: Plan) -> Result<(), ShellError> {
        let kind = plan.kind;
        let audio = &plan.audio;
        let region = plan.region;
        let backend = if let Some(notes) = &self.audio_to_midi.fixture {
            Backend::Notes(notes.clone())
        } else if kind == ConvertKind::Drums {
            Backend::Classifier
        } else {
            let folder = self.packages_folder();
            let packages = self.audio_to_midi_packages();
            let [runtime, model] = packages.as_slice() else {
                return Err(unavailable(
                    id,
                    "Harmony and Melody aren't available on this platform",
                ));
            };
            Backend::Network {
                runtime: runtime.path(&folder),
                model: model.path(&folder),
            }
        };
        let base = plan.tempo.ticks_to_seconds(plan.clip.start);
        // Spec: audio-to-midi R8. The transients the person placed or
        // moved, in seconds from the region's start.
        let hints = audio
            .transient_edits
            .added
            .iter()
            .filter(|time| **time >= region.0 && **time <= region.1)
            .map(|time| time - region.0)
            .collect();
        let worker = work::convert(work::Job {
            backend,
            media: Arc::clone(&self.media),
            source: bl_media::ContentHash::from_bytes(audio.media.0),
            region,
            kind,
            sixteenth: plan
                .tempo
                .ticks_to_seconds(plan.clip.start + Ticks(SIXTEENTH))
                - base,
            hints,
        })
        .map_err(|e| self.failed(id, e))?;
        let clip = plan.clip.id;
        self.audio_to_midi.last = Some((kind, clip));
        self.audio_to_midi.running = Some(Running { worker, plan });
        self.audio_to_midi.outcome = JobStatus::default();
        self.audio_to_midi.created = None;
        self.audio_to_midi.waiting = None;
        self.audio_to_midi.dialog = Some(ConvertDialogStatus {
            step: ConvertDialogStep::Converting,
            kind,
            clip,
        });
        self.touch();
        Ok(())
    }

    /// Takes the download's and the conversion's news. Returns whether
    /// either is still going, so a window keeps drawing its progress.
    pub(super) fn poll_audio_to_midi(&mut self) -> bool {
        self.poll_audio_to_midi_held(self.landings_wait())
    }

    /// [`Shell::poll_audio_to_midi`], with `held` set while a transaction is
    /// open: the conversion's result then stays on its worker, so it doesn't
    /// land inside a drag's undo step.
    fn poll_audio_to_midi_held(&mut self, held: bool) -> bool {
        // Spec: audio-to-midi R2. The window follows the download, and the
        // conversion starts once the packages are installed.
        if let Some((kind, clip)) = self.audio_to_midi.waiting {
            let packages = self.audio_to_midi_packages();
            let active = self.packages_active(&packages);
            if active {
                if let Some(dialog) = &mut self.audio_to_midi.dialog
                    && dialog.step == ConvertDialogStep::Packages
                {
                    dialog.step = ConvertDialogStep::Installing;
                    self.touch();
                }
            } else if self.audio_to_midi.dialog.as_ref().map(|d| d.step)
                == Some(ConvertDialogStep::Installing)
            {
                if self.audio_to_midi_packages_installed() {
                    self.audio_to_midi.waiting = None;
                    let id = ids::CONVERT;
                    let started = self
                        .conversion_plan(id, kind, clip)
                        .and_then(|plan| self.start_conversion(id, plan));
                    if let Err(error) = started {
                        self.conversion_ended(JobState::Failed, Some(error.to_string()));
                    }
                } else if let Some(dialog) = &mut self.audio_to_midi.dialog {
                    dialog.step = ConvertDialogStep::Packages;
                    self.touch();
                }
            }
        }
        let ended = self
            .audio_to_midi
            .running
            .as_mut()
            .filter(|_| !held)
            .and_then(|running| running.worker.poll());
        if let Some(ended) = ended {
            let running = self.audio_to_midi.running.take().expect("polled above");
            // A conversion that finished after Cancel was pressed is
            // cancelled all the same (R3).
            let ended = match ended {
                Ended::Done(_) if running.worker.progress().cancelled() => Ended::Cancelled,
                other => other,
            };
            match ended {
                Ended::Done(found) => {
                    if let Err(error) = self.land_conversion(&running.plan, &found) {
                        self.conversion_ended(JobState::Failed, Some(error));
                    }
                }
                Ended::Cancelled => self.conversion_ended(JobState::Cancelled, None),
                Ended::Failed(error) => self.conversion_ended(JobState::Failed, Some(error)),
            }
        }
        self.audio_to_midi.running.is_some()
            || (self.audio_to_midi.waiting.is_some() && self.packages_busy())
    }

    /// Cancels the conversion under way and forgets the window and the
    /// track of the last one, for a set that's been replaced.
    pub(super) fn drop_conversion(&mut self) {
        if let Some(running) = self.audio_to_midi.running.take() {
            running.worker.progress().cancel();
        }
        let fixture = self.audio_to_midi.fixture.take();
        self.audio_to_midi = AudioToMidiState {
            fixture,
            ..AudioToMidiState::default()
        };
    }

    fn conversion_ended(&mut self, state: JobState, error: Option<String>) {
        if let Some(error) = &error {
            self.set_message(format!("Couldn't convert the clip: {error}"), true);
        }
        self.audio_to_midi.outcome = JobStatus {
            state,
            progress: if state == JobState::Done { 1.0 } else { 0.0 },
            stage: String::new(),
            error,
        };
        self.audio_to_midi.dialog = None;
        self.touch();
    }

    /// The edit: the new track with its instrument and its clip, as one undo
    /// step named after the command (R5, R6, R9, R10, R19).
    fn land_conversion(&mut self, plan: &Plan, found: &[Found]) -> Result<(), String> {
        let doc = self.editor.document();
        let source = plan.clip.id;
        // The clip is where it is now, if the person moved it while it was
        // converted; its notes are timed as it was when the conversion began.
        let (live_start, live_length) = doc
            .clip(source)
            .map(|live| (live.start, live.length))
            .ok_or("the clip was deleted while it was converted")?;
        let source_track = doc
            .clip_track(source)
            .ok_or("the clip was deleted while it was converted")?;
        let (parent, index) = self.track_after(source_track);
        let scene: Option<SceneId> = doc.track(source_track).and_then(|track| {
            track
                .slots()
                .find(|(_, slot)| slot.id == source)
                .map(|(scene, _)| scene)
        });
        let base = self.conversion_name(&plan.clip, &plan.audio);
        let name = format!("{base} {}", plan.kind.name());
        let notes = plan.place(found);
        let count = notes.len();
        let audio = &plan.audio;
        let mut midi = MidiData::new(notes);
        // Spec: audio-to-midi R6. The source's loop, brace and markers, at
        // the same song positions.
        midi.looping = audio.looping;
        let origin = plan.origin();
        midi.start = Ticks::ZERO - origin;
        let after_start = |ticks: Ticks| (ticks > Ticks::ZERO).then_some(ticks - origin);
        midi.end = if audio.looping {
            None
        } else {
            audio
                .end
                .and_then(|end| after_start(plan.offset_of_form(end)))
        };
        if audio.looping {
            midi.loop_start = (plan.offset_of_form(audio.loop_start) - origin).max(Ticks::ZERO);
            midi.loop_end = audio
                .loop_end
                .map(|end| plan.offset_of_form(end) - origin)
                .filter(|end| *end > midi.loop_start);
        }
        let track: TrackId = self.editor.new_id();
        let clip: ClipId = self.editor.new_id();
        let session = scene.is_some();
        let midi_clip = Clip {
            id: clip,
            name: plan.clip.name.clone(),
            start: if session { Ticks::ZERO } else { live_start },
            length: live_length,
            offset: Ticks::ZERO,
            mute: false,
            data: ClipData::Midi(midi),
            envelopes: Vec::new(),
            launch: if session {
                plan.clip.launch
            } else {
                bl_model::ClipLaunch::default()
            },
            scale: bl_model::Scale::default(),
            groove: None,
            colour: plan.clip.colour,
            info_text: String::new(),
            link: None,
        };
        let look = &self.prefs.look;
        let kind = plan.kind;
        self.editor
            .transact(kind.command(), |editor| {
                let error = |e: bl_model::EditError| e.to_string();
                let rack = crate::audio_to_midi_racks::instrument_for(editor, kind);
                editor
                    .apply(Command::CreateTrack {
                        id: track,
                        kind: TrackKind::Midi,
                        name,
                        parent,
                        index,
                    })
                    .map_err(error)?;
                editor
                    .apply(Command::InsertDevice {
                        track,
                        index: 0,
                        device: rack,
                    })
                    .map_err(error)?;
                editor
                    .apply(match scene {
                        Some(scene) => Command::CreateSlotClip {
                            track,
                            scene,
                            clip: midi_clip,
                        },
                        None => Command::CreateClip {
                            track,
                            clip: midi_clip,
                        },
                    })
                    .map_err(error)?;
                // The clip keeps the source's colour; a source with none
                // gives it the track's.
                let clips = if plan.clip.colour.is_some() {
                    vec![]
                } else {
                    vec![clip]
                };
                crate::colours::colour_new(editor, look, &[track], &clips).map_err(error)
            })
            .map_err(|e| e.to_string())?;
        self.changed();
        let _ = self.arrangement.run(
            view_ids::SELECT_TRACKS,
            json!({ "tracks": [track] }),
            self.editor.document(),
        );
        self.audio_to_midi.created = Some((track, clip, count));
        if count == 0 {
            // Spec: audio-to-midi R19.
            self.set_message(
                format!("{} found no notes: the clip is empty", plan.kind.name()),
                false,
            );
        }
        self.conversion_ended(JobState::Done, None);
        Ok(())
    }

    /// The name the new track takes before the kind: the clip's, or its
    /// file's (R5).
    fn conversion_name(&self, clip: &Clip, audio: &AudioData) -> String {
        if !clip.name.is_empty() {
            return clip.name.clone();
        }
        self.media
            .entry(bl_media::ContentHash::from_bytes(audio.media.0))
            .and_then(|e| e.path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Audio".to_owned())
    }

    /// Waits until the download and the conversion under way have finished
    /// and the edit has landed, for tests and headless runs.
    pub fn wait_audio_to_midi(&mut self) {
        loop {
            let downloading = self.poll_packages();
            if !(self.poll_audio_to_midi_held(false) || downloading) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GesturePhase;
    use serde_json::json;

    /// A shell with a short audio clip, and a conversion of it under way
    /// whose worker finishes once `release` is sent.
    fn converting(name: &str) -> (Shell, ClipId, std::sync::mpsc::Sender<()>) {
        let file =
            std::env::temp_dir().join(format!("bl-a2m-unit-{name}-{}.wav", std::process::id()));
        let mut writer = bl_media::AudioWriter::create(
            &file,
            bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: 48_000,
                channels: 1,
            },
        )
        .unwrap();
        writer.write(&vec![0.25; 2 * 48_000]).unwrap();
        writer.finish().unwrap();
        let mut shell = Shell::headless();
        let placed = shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [file.to_string_lossy()], "at": 0 }),
            )
            .unwrap();
        let _ = std::fs::remove_file(&file);
        let clip: ClipId = serde_json::from_value(placed["clips"][0]["clip"].clone()).unwrap();
        shell.use_transcription_fixture(Vec::new());
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let worker = Worker::start("bl-test-audio-to-midi", move |progress| {
            let _ = wait.recv();
            if progress.cancelled() {
                return Ended::Cancelled;
            }
            Ended::Done(vec![Found {
                start: 0.5,
                end: 0.9,
                pitch: 60,
                velocity: 100,
                confidence: 0.5,
            }])
        })
        .unwrap();
        let plan = shell
            .conversion_plan(ids::CONVERT, ConvertKind::Melody, clip)
            .unwrap();
        shell.audio_to_midi.last = Some((ConvertKind::Melody, clip));
        shell.audio_to_midi.dialog = Some(ConvertDialogStatus {
            step: ConvertDialogStep::Converting,
            kind: ConvertKind::Melody,
            clip,
        });
        shell.audio_to_midi.running = Some(Running { worker, plan });
        (shell, clip, release)
    }

    fn poll_until_landed(shell: &mut Shell) {
        for _ in 0..5_000 {
            shell.poll();
            if shell.audio_to_midi.running.is_none() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("the conversion never landed");
    }

    // Spec: audio-to-midi R3. Cancel stops a conversion and changes nothing
    // in the set; the window closes, and the status says it was cancelled.
    #[test]
    fn a_cancelled_conversion_changes_nothing() {
        let (mut shell, _clip, release) = converting("cancel");
        let before = shell.editor().document().clone();
        let steps = shell.editor().undo_history().count();
        let status = shell.run(ids::CANCEL, json!({})).unwrap();
        assert!(status["dialog"].is_null());
        release.send(()).unwrap();
        poll_until_landed(&mut shell);
        assert_eq!(*shell.editor().document(), before);
        assert_eq!(shell.editor().undo_history().count(), steps);
        let status = shell.run(ids::STATUS, json!({})).unwrap();
        assert_eq!(status["conversion"]["state"], "cancelled");
        assert!(status["created_track"].is_null());
    }

    // Spec: audio-to-midi R26. A second conversion is refused while one runs,
    // and the first goes on.
    #[test]
    fn a_second_conversion_is_refused_while_one_runs() {
        let (mut shell, clip, release) = converting("second");
        let error = shell
            .run(ids::CONVERT, json!({ "clip": clip, "kind": "drums" }))
            .unwrap_err();
        assert!(error.to_string().contains("already under way"), "{error}");
        let status = shell.run(ids::STATUS, json!({})).unwrap();
        assert_eq!(status["conversion"]["state"], "running");
        assert_eq!(status["kind"], "melody");
        assert_eq!(status["dialog"]["step"], "converting");
        release.send(()).unwrap();
        poll_until_landed(&mut shell);
        let status = shell.run(ids::STATUS, json!({})).unwrap();
        assert_eq!(status["conversion"]["state"], "done");
    }

    // Spec: audio-to-midi R10. A conversion that finishes while the person
    // drags a clip lands when the drag ends, not inside the drag's undo step.
    #[test]
    fn a_conversion_that_finishes_during_a_drag_lands_after_it() {
        let (mut shell, clip, release) = converting("drag");
        shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .unwrap();
        let frame = |by: i64| crate::Invocation::new("arrangement.move_clips", json!({ "by": by }));
        shell
            .gesture(GesturePhase::Update, Some(&frame(960)))
            .unwrap();
        release.send(()).unwrap();
        for _ in 0..2_000 {
            shell.poll();
            std::thread::yield_now();
        }
        assert!(
            shell.audio_to_midi.running.is_some(),
            "the result waits for the drag"
        );
        shell
            .gesture(GesturePhase::Commit, Some(&frame(1_920)))
            .unwrap();
        poll_until_landed(&mut shell);
        let (track, made, notes) = shell.audio_to_midi.created.expect("the track landed");
        let document = shell.editor().document();
        assert!(document.track(track).is_some());
        assert_eq!(
            document.clip(made).map(|c| c.start),
            document.clip(clip).map(|c| c.start)
        );
        assert_eq!(notes, 1);
    }

    // Spec: audio-to-midi R3. A set that is replaced drops the conversion
    // under way: a new set gets neither its track nor its window.
    #[test]
    fn a_new_set_drops_the_conversion_under_way() {
        let (mut shell, _clip, release) = converting("replaced");
        shell.run("set.new", json!({})).unwrap();
        let fresh = shell.editor().document().clone();
        assert!(shell.audio_to_midi.running.is_none());
        assert!(shell.audio_to_midi_dialog().is_none());
        let _ = release.send(());
        for _ in 0..2_000 {
            shell.poll();
            std::thread::yield_now();
        }
        assert_eq!(*shell.editor().document(), fresh);
    }
}

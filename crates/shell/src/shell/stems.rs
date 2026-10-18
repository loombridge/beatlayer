//! Separate Stems to New Audio Tracks through the shell (IMP-11): the
//! `stems.*` commands, the dialog's state, and the edit that lands once the
//! stems are written. The work itself is in [`crate::stems`]; the behaviour
//! is in the stem separation spec.

use std::cell::RefCell;
use std::rc::Rc;

use bl_analysis::stems::{Quality, Stem};
use bl_model::{Clip, ClipData, ClipId, Command, ContentHash, TrackId, TrackKind};
use bl_shell_spec::stems::{
    self as stems_ids, DialogStatus, DialogStep, JobState, JobStatus, OpenArgs, PackageStatus,
    SeparateArgs, StemKind, StemQuality, StemsStatus,
};

use super::*;
use crate::processed::{Kind, Request};
use crate::stems::{Ended, Separated, Worker};

/// Stem separation's state in the shell.
#[derive(Default)]
pub(super) struct StemsState {
    /// The packages' download, while it runs.
    install: Option<Worker<()>>,
    /// How the last download went, or how this one is going.
    install_status: JobStatus,
    /// The separation, while it runs.
    separation: Option<Running>,
    /// How the last separation went.
    separation_status: JobStatus,
    /// The tracks the last separation made.
    created: Vec<TrackId>,
    /// The dialog, while it's open.
    dialog: Option<DialogStatus>,
}

/// A separation under way.
struct Running {
    worker: Worker<Separated>,
    args: SeparateArgs,
}

/// One file a separation writes.
struct Output {
    /// The parts it holds.
    parts: Vec<StemKind>,
    /// Whether it gets a track.
    track: bool,
}

/// The files written so far, until every one has landed.
struct Landing {
    args: SeparateArgs,
    outputs: Vec<Output>,
    written: Vec<Option<ContentHash>>,
}

fn stem_of(kind: StemKind) -> Stem {
    match kind {
        StemKind::Vocals => Stem::Vocals,
        StemKind::Drums => Stem::Drums,
        StemKind::Bass => Stem::Bass,
        StemKind::Others => Stem::Others,
    }
}

/// A file's or track's name for the parts it holds: "Vocals", or
/// "Vocals + Drums" for merged parts.
fn parts_name(parts: &[StemKind]) -> String {
    parts
        .iter()
        .map(|p| p.name())
        .collect::<Vec<_>>()
        .join(" + ")
}

impl Shell {
    /// The folder the stem separation packages install into: `Packages` in
    /// Beatlayer's application-data folder.
    #[must_use]
    pub fn packages_folder(&self) -> PathBuf {
        self.config_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("Beatlayer"))
            .join("Packages")
    }

    /// Separate Stems' dialog, if it's open.
    #[must_use]
    pub fn stems_dialog(&self) -> Option<DialogStatus> {
        self.stems.dialog
    }

    /// Whether every stem separation package is installed.
    #[must_use]
    pub fn stem_packages_installed(&self) -> bool {
        let folder = self.packages_folder();
        let packages = crate::stems::packages();
        !packages.is_empty() && packages.iter().all(|p| p.installed(&folder))
    }

    /// Stem separation's state, as `stems.status` reads it.
    #[must_use]
    pub fn stems_status(&self) -> StemsStatus {
        let folder = self.packages_folder();
        let state = &self.stems;
        let running = |worker: &crate::stems::Progress| JobStatus {
            state: JobState::Running,
            progress: worker.fraction(),
            stage: worker.stage(),
            error: None,
        };
        StemsStatus {
            installed: self.stem_packages_installed(),
            folder: folder.to_string_lossy().into_owned(),
            packages: crate::stems::packages()
                .iter()
                .map(|p| PackageStatus {
                    name: p.name.to_owned(),
                    purpose: p.purpose.to_owned(),
                    version: p.version.to_owned(),
                    url: p.project.to_owned(),
                    size: p.size,
                    installed: p.installed(&folder),
                })
                .collect(),
            install: state
                .install
                .as_ref()
                .map_or_else(|| state.install_status.clone(), |w| running(w.progress())),
            separation: state.separation.as_ref().map_or_else(
                || state.separation_status.clone(),
                |r| running(r.worker.progress()),
            ),
            created_tracks: state.created.clone(),
            dialog: state.dialog,
        }
    }

    /// The audio clip in the Arrangement `clip` names, or the selected one.
    fn stems_clip(&self, clip: Option<ClipId>) -> Result<ClipId, String> {
        let clip = match clip {
            Some(clip) => clip,
            None => match self.arrangement.selected_clips() {
                [clip] => *clip,
                [] => return Err("no clip is selected".to_owned()),
                _ => return Err("choose one clip to separate".to_owned()),
            },
        };
        let doc = self.editor.document();
        match doc.clip(clip).map(|c| &c.data) {
            Some(ClipData::Audio(_)) => Ok(clip),
            Some(_) => Err("only an audio clip can be separated into stems".to_owned()),
            None => Err(format!("no clip in the Arrangement has the ID {}", clip.0)),
        }
    }

    /// Whether a `stems.*` command can run now.
    pub(super) fn can_stems(&self, id: &str, args: &Value) -> bool {
        match id {
            stems_ids::OPEN => {
                let clip = serde_json::from_value::<OpenArgs>(Self::object(args.clone()))
                    .ok()
                    .and_then(|a| a.clip);
                !crate::stems::packages().is_empty() && self.stems_clip(clip).is_ok()
            }
            stems_ids::CLOSE => self.stems.dialog.is_some(),
            stems_ids::CANCEL => self.stems.install.is_some() || self.stems.separation.is_some(),
            _ => true,
        }
    }

    pub(super) fn run_stems(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        match id {
            stems_ids::STATUS => {}
            stems_ids::OPEN => {
                let OpenArgs { clip } = Self::args(id, args)?;
                if crate::stems::packages().is_empty() {
                    return Err(unavailable(
                        "stem separation isn't available on this platform".to_owned(),
                    ));
                }
                let clip = self.stems_clip(clip).map_err(unavailable)?;
                let step = if self.stems.separation.is_some() {
                    DialogStep::Separating
                } else if self.stems.install.is_some() {
                    DialogStep::Installing
                } else if self.stem_packages_installed() {
                    DialogStep::Options
                } else {
                    DialogStep::Packages
                };
                self.stems.dialog = Some(DialogStatus { step, clip });
                self.touch();
            }
            stems_ids::CLOSE => {
                self.stems.dialog = None;
                self.touch();
            }
            stems_ids::INSTALL => self.install_stem_packages(id)?,
            stems_ids::SEPARATE => {
                let args: SeparateArgs = Self::args(id, args)?;
                self.start_separation(id, args)?;
            }
            stems_ids::CANCEL => {
                if let Some(install) = &self.stems.install {
                    install.progress().cancel();
                }
                if let Some(running) = &self.stems.separation {
                    running.worker.progress().cancel();
                }
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.stems_status()))
    }

    fn install_stem_packages(&mut self, id: &str) -> Result<(), ShellError> {
        if crate::stems::packages().is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "stem separation isn't available on this platform".to_owned(),
            });
        }
        if self.stems.install.is_some() || self.stem_packages_installed() {
            return Ok(());
        }
        let worker =
            crate::stems::install(self.packages_folder()).map_err(|e| self.failed(id, e))?;
        self.stems.install = Some(worker);
        self.stems.install_status = JobStatus::default();
        if let Some(dialog) = &mut self.stems.dialog {
            dialog.step = DialogStep::Installing;
        }
        self.touch();
        Ok(())
    }

    fn start_separation(&mut self, id: &str, mut args: SeparateArgs) -> Result<(), ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        if self.stems.separation.is_some() {
            return Err(unavailable("a separation is already under way"));
        }
        if !self.stem_packages_installed() {
            return Err(unavailable(
                "the stem separation packages aren't installed: run `stems.install` first",
            ));
        }
        args.stems.sort();
        args.stems.dedup();
        if args.stems.is_empty() {
            return Err(unavailable("choose at least one part to separate"));
        }
        if args.merge && !(2..=3).contains(&args.stems.len()) {
            return Err(unavailable(
                "Merge to Single Track needs two or three parts",
            ));
        }
        let clip = self
            .stems_clip(Some(args.clip))
            .map_err(|reason| unavailable(&reason))?;
        let doc = self.editor.document();
        let Some(ClipData::Audio(audio)) = doc.clip(clip).map(|c| &c.data) else {
            return Err(unavailable(
                "only an audio clip can be separated into stems",
            ));
        };
        let source = bl_media::ContentHash::from_bytes(audio.media.0);
        // Spec: stem-separation R8. The Start marker to the End marker, or
        // through the loop brace.
        let length = self
            .media
            .info(source)
            .ok()
            .and_then(|info| {
                info.frames
                    .map(|frames| frames as f64 / f64::from(info.sample_rate.max(1)))
            })
            .unwrap_or(f64::MAX);
        let mut start = audio.start;
        let mut end = audio.end.unwrap_or(length);
        if audio.looping {
            start = start.min(audio.loop_start);
            end = end.max(audio.loop_end.unwrap_or(length));
        }
        let region = (start, end.min(length));
        // Spec: stem-separation R6. Playback stops first.
        if self.audio.transport_state().playing {
            let _ = self.audio.send_transport(bl_engine::TransportCommand::Stop);
        }
        let worker = crate::stems::separate(crate::stems::Job {
            packages: self.packages_folder(),
            media: Arc::clone(&self.media),
            source,
            region,
            quality: match args.quality {
                StemQuality::HighSpeed => Quality::HighSpeed,
                StemQuality::HighQuality => Quality::HighQuality,
            },
        })
        .map_err(|e| self.failed(id, e))?;
        self.stems.separation = Some(Running { worker, args });
        self.stems.separation_status = JobStatus::default();
        self.stems.created.clear();
        if let Some(dialog) = &mut self.stems.dialog {
            dialog.step = DialogStep::Separating;
        }
        self.touch();
        Ok(())
    }

    /// Takes the download's and the separation's news. Returns whether
    /// either is still going, so a window keeps drawing its progress.
    pub(super) fn poll_stems(&mut self) -> bool {
        if let Some(ended) = self.stems.install.as_mut().and_then(Worker::poll) {
            self.stems.install = None;
            let (state, error) = match ended {
                Ended::Done(()) => (JobState::Done, None),
                Ended::Cancelled => (JobState::Cancelled, None),
                Ended::Failed(error) => (JobState::Failed, Some(error)),
            };
            if let Some(error) = &error {
                self.set_message(error.clone(), true);
            }
            self.stems.install_status = JobStatus {
                state,
                progress: if state == JobState::Done { 1.0 } else { 0.0 },
                stage: String::new(),
                error,
            };
            if let Some(dialog) = &mut self.stems.dialog {
                // Spec: stem-separation R4. Once installed, the dialog
                // offers the stems.
                dialog.step = if state == JobState::Done {
                    DialogStep::Options
                } else {
                    DialogStep::Packages
                };
            }
            self.touch();
        }
        let ended = self
            .stems
            .separation
            .as_mut()
            .and_then(|running| running.worker.poll());
        if let Some(ended) = ended {
            let running = self.stems.separation.take().expect("polled above");
            match ended {
                Ended::Done(separated) => {
                    if let Err(error) = self.write_stems(running.args, &separated) {
                        self.separation_ended(JobState::Failed, Some(error));
                    }
                }
                Ended::Cancelled => self.separation_ended(JobState::Cancelled, None),
                Ended::Failed(error) => self.separation_ended(JobState::Failed, Some(error)),
            }
        }
        self.stems.install.is_some() || self.stems.separation.is_some()
    }

    fn separation_ended(&mut self, state: JobState, error: Option<String>) {
        if let Some(error) = &error {
            self.set_message(format!("Couldn't separate stems: {error}"), true);
        }
        self.stems.separation_status = JobStatus {
            state,
            progress: if state == JobState::Done { 1.0 } else { 0.0 },
            stage: String::new(),
            error,
        };
        match (state, &mut self.stems.dialog) {
            (JobState::Cancelled, Some(dialog)) => dialog.step = DialogStep::Options,
            _ => self.stems.dialog = None,
        }
        self.touch();
    }

    /// Writes the separated files, and makes the tracks once every one is
    /// whole (R9 to R11).
    fn write_stems(&mut self, args: SeparateArgs, separated: &Separated) -> Result<(), String> {
        let doc = self.editor.document();
        let clip = doc
            .clip(args.clip)
            .ok_or_else(|| "the clip was deleted while it was separated".to_owned())?;
        let base = self.stems_base_name(clip);
        // Spec: stem-separation R10. Merged, every part's own file is
        // written too, and only the merged file gets a track.
        let mut outputs: Vec<Output> = args
            .stems
            .iter()
            .map(|kind| Output {
                parts: vec![*kind],
                track: !args.merge,
            })
            .collect();
        if args.merge {
            outputs.push(Output {
                parts: args.stems.clone(),
                track: true,
            });
        }
        let landing = Rc::new(RefCell::new(Landing {
            args,
            written: outputs.iter().map(|_| None).collect(),
            outputs: Vec::new(),
        }));
        let requests: Vec<Request> = outputs
            .iter()
            .map(|output| {
                let parts: Vec<Stem> = output.parts.iter().copied().map(stem_of).collect();
                let frames = crate::stems::file_frames(separated, &parts);
                Request {
                    kind: Kind::Stems,
                    name: format!("{base} {}.wav", parts_name(&output.parts)),
                    spec: bl_media::EncodeSpec {
                        file_type: bl_media::FileType::Wav,
                        bit_depth: bl_media::BitDepth::Float32,
                        dither: bl_media::Dither::None,
                        sample_rate: bl_analysis::stems::RATE,
                        channels: 2,
                    },
                    frames: (frames.len() / 2) as u64,
                    produce: Box::new(move |sink| sink.write(&frames)),
                }
            })
            .collect();
        landing.borrow_mut().outputs = outputs;
        for (index, request) in requests.into_iter().enumerate() {
            let landing = Rc::clone(&landing);
            self.start_processed(request, move |shell, written| {
                let ready = {
                    let mut landing = landing.borrow_mut();
                    landing.written[index] = Some(ContentHash(*written.hash.as_bytes()));
                    landing.written.iter().all(Option::is_some)
                };
                if ready {
                    shell.land_stems(&landing.borrow())?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// The name the new files and group take: the clip's, or its file's.
    fn stems_base_name(&self, clip: &Clip) -> String {
        if !clip.name.is_empty() {
            return clip.name.clone();
        }
        let ClipData::Audio(audio) = &clip.data else {
            return "Stems".to_owned();
        };
        self.media
            .entry(bl_media::ContentHash::from_bytes(audio.media.0))
            .and_then(|e| e.path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Stems".to_owned())
    }

    /// The edit: the new tracks with their clips, and the source clip
    /// deactivated, as one undo step (R9, R11).
    fn land_stems(&mut self, landing: &Landing) -> Result<(), String> {
        let source_clip = landing.args.clip;
        let doc = self.editor.document();
        let clip = doc
            .clip(source_clip)
            .cloned()
            .ok_or_else(|| "the clip was deleted while it was separated".to_owned())?;
        let track = doc
            .clip_track(source_clip)
            .ok_or_else(|| "the clip's track was deleted".to_owned())?;
        let base = self.stems_base_name(&clip);
        let parent = doc.parent(track);
        let siblings: Vec<TrackId> = match parent {
            Some(group) => doc
                .track(group)
                .map(|g| g.children().collect())
                .unwrap_or_default(),
            None => doc.top_level().collect(),
        };
        let below = siblings
            .iter()
            .position(|t| *t == track)
            .map_or(0, |i| i + 1);
        let tracks: Vec<(String, ContentHash)> = landing
            .outputs
            .iter()
            .zip(&landing.written)
            .filter(|(output, _)| output.track)
            .filter_map(|(output, hash)| hash.map(|hash| (parts_name(&output.parts), hash)))
            .collect();
        let mut created = Vec::new();
        self.editor
            .transact(stems_ids::STEP, |editor| {
                // Spec: stem-separation R9. More than one part goes in a
                // new group below the clip's track; one part, or a merge,
                // gets a track of its own there.
                let (parent, first) = if tracks.len() > 1 {
                    let group: TrackId = editor.new_id();
                    editor.apply(Command::CreateTrack {
                        id: group,
                        kind: TrackKind::Group,
                        name: base.clone(),
                        parent,
                        index: below,
                    })?;
                    created.push(group);
                    (Some(group), 0)
                } else {
                    (parent, below)
                };
                for (offset, (name, hash)) in tracks.iter().enumerate() {
                    let id: TrackId = editor.new_id();
                    editor.apply(Command::CreateTrack {
                        id,
                        kind: TrackKind::Audio,
                        name: name.clone(),
                        parent,
                        index: first + offset,
                    })?;
                    created.push(id);
                    // Spec: stem-separation R11. A copy of the clip, playing
                    // its part: the markers, warping, gain and envelopes
                    // carry over, since the file lines up with the clip's.
                    let mut copy = clip.clone();
                    copy.id = editor.new_id();
                    copy.name.clone_from(name);
                    copy.mute = false;
                    if let ClipData::Audio(audio) = &mut copy.data {
                        audio.media = *hash;
                    }
                    editor.apply(Command::CreateClip {
                        track: id,
                        clip: copy,
                    })?;
                }
                // Spec: stem-separation R11. The original is deactivated.
                editor.apply(Command::SetClipMute {
                    clip: source_clip,
                    mute: true,
                })?;
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| e.to_string())?;
        self.changed();
        self.stems.created = created;
        self.separation_ended(JobState::Done, None);
        Ok(())
    }

    /// Waits until the download and the separation under way have finished
    /// and the stems' edit has landed, for tests and headless runs.
    pub fn wait_stems(&mut self) {
        while self.poll_stems() || self.is_processing() {
            self.poll_processed();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

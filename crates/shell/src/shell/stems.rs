//! Separate Stems to New Audio Tracks through the shell (IMP-11): the
//! `stems.*` commands, the dialog's state, and the edit that lands once the
//! stems are written. The work itself is in [`crate::stems`]; the behaviour
//! is in the stem separation spec.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use bl_analysis::stems::{Quality, Stem};
use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, TrackId, TrackKind,
};
use bl_shell_spec::browser::{AddToArrangementArgs, ItemKind};
use bl_shell_spec::stems::{
    self as stems_ids, DialogStatus, DialogStep, JobState, JobStatus, OpenArgs, SeparateArgs,
    StemKind, StemQuality, StemSource, StemsStatus,
};

use super::audio_editing::{LONGEST, beat_markers, consolidate_document, consolidated_audio};
use super::*;
use crate::arrangement::placing;
use crate::browser::{arrange, listing};
use crate::packages::Package;
use crate::processed::{Kind, Request};
use crate::stems::{Backend, Ended, Input, Separated, Worker};

/// Stem separation's state in the shell.
#[derive(Default)]
pub(super) struct StemsState {
    /// The separation, while it runs.
    separation: Option<Running>,
    /// How the last separation went.
    separation_status: JobStatus,
    /// What the last separation ran on.
    provider: String,
    /// The tracks the last separation made.
    created: Vec<TrackId>,
    /// The clips the last separation made.
    created_clips: Vec<ClipId>,
    /// The dialog, while it's open.
    dialog: Option<DialogStatus>,
    /// Whether a generated stand-in does the network's work, for tests.
    fixture: bool,
    /// Whether the separation's files are being written, which its tracks
    /// wait on.
    writing: bool,
}

/// A separation under way.
struct Running {
    worker: Worker<Vec<Separated>>,
    args: SeparateArgs,
    plan: Plan,
}

/// What a separation was started on, as the set held it then.
struct Plan {
    source: StemSource,
    /// Where a file's clips start: the insert marker.
    insert: Ticks,
    /// The track a file's tracks go after, if one was selected.
    after: Option<TrackId>,
    /// The audio separated, in the order of the separation's inputs.
    pieces: Vec<Piece>,
}

/// One piece of audio a separation separates: a clip's region, a file, or
/// one track's part of a time selection.
struct Piece {
    /// The track a time selection's audio comes from.
    track: Option<TrackId>,
    /// The name its files and its group take (R28).
    base: String,
}

/// One file a separation writes.
struct Output {
    /// The parts it holds.
    parts: Vec<StemKind>,
    /// Whether it gets a track.
    track: bool,
}

/// What one piece wrote: its files, and where each is once written.
struct Item {
    outputs: Vec<Output>,
    written: Vec<Option<(ContentHash, PathBuf)>>,
}

/// The files written so far, until every one has landed.
struct Landing {
    plan: Plan,
    items: Vec<Item>,
}

/// What a landing made.
#[derive(Default)]
struct Made {
    tracks: Vec<TrackId>,
    clips: Vec<ClipId>,
}

/// What a landing reads from the shell.
struct Env<'a> {
    media: &'a bl_media::MediaStore,
    edge_fades: bool,
    warping: crate::auto_warp::Warping<'a>,
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

/// Where new tracks go after `track`: its group and place in it.
fn after(doc: &Document, track: TrackId) -> (Option<TrackId>, usize) {
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
    (parent, below)
}

/// The audio clips of `track` that play in `start..end`.
fn playing_in(doc: &Document, track: TrackId, start: Ticks, end: Ticks) -> Vec<ClipId> {
    doc.track(track)
        .map(|track| {
            track
                .clips()
                .filter(|clip| {
                    !clip.mute
                        && matches!(clip.data, ClipData::Audio(_))
                        && clip.start < end
                        && clip.end() > start
                })
                .map(|clip| clip.id)
                .collect()
        })
        .unwrap_or_default()
}

impl Shell {
    /// Separate Stems' dialog, if it's open.
    #[must_use]
    pub fn stems_dialog(&self) -> Option<DialogStatus> {
        self.stems.dialog.clone()
    }

    /// Lets a generated stand-in do the network's work, so a test separates
    /// without the packages.
    #[doc(hidden)]
    pub fn use_stem_fixture(&mut self) {
        self.stems.fixture = true;
    }

    /// The packages stem separation needs (R15).
    fn stems_packages(&self) -> Vec<Package> {
        self.feature_packages(crate::stems::FEATURE)
    }

    /// Whether every stem separation package is installed.
    #[must_use]
    pub fn stem_packages_installed(&self) -> bool {
        let folder = self.packages_folder();
        let packages = self.stems_packages();
        !packages.is_empty() && packages.iter().all(|p| p.installed(&folder))
    }

    /// Stem separation's state, as `stems.status` reads it.
    #[must_use]
    pub fn stems_status(&self) -> StemsStatus {
        let state = &self.stems;
        let packages = self.stems_packages();
        let running = |worker: &crate::stems::Progress| JobStatus {
            state: JobState::Running,
            progress: worker.fraction(),
            stage: worker.stage(),
            error: None,
        };
        StemsStatus {
            installed: self.stem_packages_installed(),
            folder: self.packages_folder().to_string_lossy().into_owned(),
            packages: packages.iter().map(|p| self.package_status(p)).collect(),
            install: self.packages_job(&packages),
            separation: match &state.separation {
                Some(running_now) => running(running_now.worker.progress()),
                // The network is done and the tracks wait for the files.
                None if state.writing => JobStatus {
                    state: JobState::Running,
                    progress: 1.0,
                    stage: "Writing the files".to_owned(),
                    error: None,
                },
                None => state.separation_status.clone(),
            },
            provider: state.separation.as_ref().map_or_else(
                || state.provider.clone(),
                |r| r.worker.progress().provider(),
            ),
            created_tracks: state.created.clone(),
            created_clips: state.created_clips.clone(),
            dialog: state.dialog.clone(),
        }
    }

    /// What is selected to separate (R22): one audio clip in the
    /// Arrangement, a time selection on audio tracks, one audio clip shown
    /// from the Session View, or one audio file in the browser, in that
    /// order.
    fn stems_selected(&self) -> Result<StemSource, String> {
        match self.arrangement.selected_clips() {
            [clip] => return Ok(StemSource::Clip { clip: *clip }),
            [] => {}
            _ => return Err("choose one clip to separate".to_owned()),
        }
        let doc = self.editor.document();
        if let Some((start, end)) = self.arrangement.time_selection() {
            let tracks: Vec<TrackId> = self
                .arrangement
                .selected_tracks()
                .iter()
                .copied()
                .filter(|track| !playing_in(doc, *track, start, end).is_empty())
                .collect();
            if !tracks.is_empty() {
                return Ok(StemSource::TimeSelection { tracks, start, end });
            }
        }
        if let Some(clip) = self.clip_view.shown_audio_clip(doc, &self.arrangement) {
            return Ok(StemSource::Clip { clip });
        }
        match self.browser_view.selected_audio().as_slice() {
            [file] => Ok(StemSource::File {
                path: file.to_string_lossy().into_owned(),
            }),
            _ => Err(
                "select one audio clip, a time selection on audio tracks or one audio file first"
                    .to_owned(),
            ),
        }
    }

    /// The source named, or the selected one, with the pieces it separates.
    fn stems_source(&self, source: Option<StemSource>) -> Result<(StemSource, Vec<Piece>), String> {
        let source = match source {
            Some(source) => source,
            None => self.stems_selected()?,
        };
        let doc = self.editor.document();
        let pieces = match &source {
            StemSource::Clip { clip } => {
                let clip = *clip;
                match doc.clip(clip) {
                    Some(held) if matches!(held.data, ClipData::Audio(_)) => {
                        vec![Piece {
                            track: None,
                            base: self.stems_base_name(held),
                        }]
                    }
                    Some(_) => {
                        return Err("only an audio clip can be separated into stems".to_owned());
                    }
                    None => return Err(format!("no clip has the ID {}", clip.0)),
                }
            }
            StemSource::File { path } => {
                let path = Path::new(path);
                let audio = path
                    .file_name()
                    .and_then(|name| listing::file_kind(&name.to_string_lossy()))
                    == Some(ItemKind::Audio);
                if !path.is_file() || !audio {
                    return Err(format!("{} isn't an audio file", path.display()));
                }
                vec![Piece {
                    track: None,
                    base: path
                        .file_stem()
                        .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
                }]
            }
            StemSource::TimeSelection { tracks, start, end } => {
                if end <= start {
                    return Err("the time selection is empty".to_owned());
                }
                // Spec: stem-separation R25, R31. Tracks with no audio in the
                // range are skipped; MIDI tracks hold none.
                let mut seen = Vec::new();
                let pieces: Vec<Piece> = tracks
                    .iter()
                    .filter(|track| {
                        // A track named twice is one source.
                        let new = !seen.contains(*track);
                        seen.push(**track);
                        new
                    })
                    .filter(|track| {
                        doc.track(**track).map(|t| t.kind()) == Some(TrackKind::Audio)
                            && !playing_in(doc, **track, *start, *end).is_empty()
                    })
                    .map(|track| {
                        let bars = &doc.settings().time_signatures;
                        let name = doc.track(*track).map(|t| t.name()).unwrap_or_default();
                        Piece {
                            track: Some(*track),
                            base: format!(
                                "{name} {}-{}",
                                bars.bar_at(*start).0 + 1,
                                bars.bar_at(*end).0 + 1
                            ),
                        }
                    })
                    .collect();
                if pieces.is_empty() {
                    return Err("the time selection holds no audio on audio tracks".to_owned());
                }
                pieces
            }
        };
        Ok((source, pieces))
    }

    /// Whether a `stems.*` command can run now.
    pub(super) fn can_stems(&self, id: &str, args: &Value) -> bool {
        match id {
            stems_ids::OPEN => {
                let source = serde_json::from_value::<OpenArgs>(Self::object(args.clone()))
                    .ok()
                    .and_then(OpenArgs::source);
                !self.stems_packages().is_empty() && self.stems_source(source).is_ok()
            }
            stems_ids::CLOSE => self.stems.dialog.is_some(),
            stems_ids::CANCEL => {
                self.packages_active(&self.stems_packages()) || self.stems.separation.is_some()
            }
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
                let args: OpenArgs = Self::args(id, args)?;
                if self.stems_packages().is_empty() {
                    return Err(unavailable(
                        "stem separation isn't available on this platform".to_owned(),
                    ));
                }
                let (source, _) = self.stems_source(args.source()).map_err(unavailable)?;
                let step = if self.stems.separation.is_some() {
                    DialogStep::Separating
                } else if self.packages_active(&self.stems_packages()) {
                    DialogStep::Installing
                } else if self.stem_packages_installed() {
                    DialogStep::Options
                } else {
                    DialogStep::Packages
                };
                self.stems.dialog = Some(DialogStatus { step, source });
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
                let packages = self.stems_packages();
                self.cancel_packages(&packages);
                if let Some(running) = &self.stems.separation {
                    running.worker.progress().cancel();
                }
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.stems_status()))
    }

    /// Runs `packages.install` for stem separation's packages (R20).
    fn install_stem_packages(&mut self, id: &str) -> Result<(), ShellError> {
        let packages = self.stems_packages();
        if packages.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "stem separation isn't available on this platform".to_owned(),
            });
        }
        if self.packages_active(&packages) || self.stem_packages_installed() {
            return Ok(());
        }
        self.request_packages(packages)
            .map_err(|e| self.failed(id, e))?;
        if let Some(dialog) = &mut self.stems.dialog {
            dialog.step = DialogStep::Installing;
        }
        self.touch();
        Ok(())
    }

    /// What a clip's separation reads: its file from its Start marker to its
    /// End marker, or through the loop (R8).
    fn clip_input(&self, clip: ClipId) -> Result<Input, String> {
        let doc = self.editor.document();
        let Some(ClipData::Audio(audio)) = doc.clip(clip).map(|c| &c.data) else {
            return Err("only an audio clip can be separated into stems".to_owned());
        };
        let source = bl_media::ContentHash::from_bytes(audio.media.0);
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
        Ok(Input::Region {
            source,
            region: (start, end.min(length)),
        })
    }

    /// What a time selection's separation reads for `track` (R25): what its
    /// clips play over the range, rendered offline without the track's
    /// devices or mixer, at 44.1 kHz.
    ///
    /// shortcut: the render runs on the calling thread and is held in
    /// memory, as Consolidate's is; upgrade when a person separates minutes
    /// of selection.
    fn selection_input(&self, track: TrackId, start: Ticks, end: Ticks) -> Result<Input, String> {
        let document = consolidate_document(self.editor.document(), track, None)?;
        let rate = self.engine_rate();
        let spec = crate::export::Render {
            start,
            length: crate::export::Length::To(end),
            sample_rate: bl_analysis::stems::RATE,
            engine_rate: rate,
            block: crate::export::BLOCK,
        };
        let cancel = AtomicBool::new(false);
        let mut rendered: Vec<f32> = Vec::new();
        {
            let mut sink = |samples: &[f32]| {
                rendered.extend_from_slice(samples);
                Ok(())
            };
            crate::export::render(
                &document,
                self.engine_media(),
                self.plugin_hosts(),
                &spec,
                LONGEST,
                &cancel,
                &mut |_| {},
                &mut sink,
                None,
            )
            .map_err(|error| error.to_string())?;
        }
        let (left, right): (Vec<f32>, Vec<f32>) = rendered
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip();
        if left.is_empty() {
            return Err("the time selection holds no audio".to_owned());
        }
        Ok(Input::Frames([left, right]))
    }

    fn start_separation(&mut self, id: &str, mut args: SeparateArgs) -> Result<(), ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        if self.stems.separation.is_some() {
            return Err(unavailable("a separation is already under way"));
        }
        if !self.stems.fixture && !self.stem_packages_installed() {
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
        let Some(source) = args.source() else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "name what to separate: `source`, or `clip`".to_owned(),
            });
        };
        let (source, pieces) = self
            .stems_source(Some(source))
            .map_err(|reason| unavailable(&reason))?;
        let mut insert = Ticks::ZERO;
        let mut after = None;
        let inputs: Vec<Input> = match &source {
            // Spec: stem-separation R8. The Start marker to the End marker,
            // or through the loop brace.
            StemSource::Clip { clip } => vec![self.clip_input(*clip).map_err(|e| unavailable(&e))?],
            // Spec: stem-separation R24. The whole file.
            StemSource::File { path } => {
                let (file, _, seconds) = arrange::prepare_for_slot(&self.media, Path::new(path))
                    .map_err(|_| unavailable("the file couldn't be read as audio"))?;
                insert = self.arrangement.insert_marker();
                after = self.arrangement.selected_tracks().first().copied();
                vec![Input::Region {
                    source: bl_media::ContentHash::from_bytes(file.hash.0),
                    region: (0.0, seconds),
                }]
            }
            StemSource::TimeSelection { start, end, .. } => pieces
                .iter()
                .filter_map(|piece| piece.track)
                .map(|track| self.selection_input(track, *start, *end))
                .collect::<Result<_, _>>()
                .map_err(|e| unavailable(&e))?,
        };
        // Spec: stem-separation R6. Playback stops first.
        if self.audio.transport_state().playing {
            let _ = self.audio.send_transport(bl_engine::TransportCommand::Stop);
        }
        let backend = if self.stems.fixture {
            Backend::Fixture
        } else {
            let folder = self.packages_folder();
            let packages = self.stems_packages();
            let [runtime, model] = packages.as_slice() else {
                return Err(unavailable(
                    "stem separation isn't available on this platform",
                ));
            };
            Backend::Packages {
                runtime: runtime.path(&folder),
                model: model.path(&folder),
                cache: self.cache_folder(),
            }
        };
        let worker = crate::stems::separate(crate::stems::Job {
            backend,
            media: Arc::clone(&self.media),
            inputs,
            quality: match args.quality {
                StemQuality::HighSpeed => Quality::HighSpeed,
                StemQuality::HighQuality => Quality::HighQuality,
            },
        })
        .map_err(|e| self.failed(id, e))?;
        let plan = Plan {
            source,
            insert,
            after,
            pieces,
        };
        self.stems.separation = Some(Running { worker, args, plan });
        self.stems.separation_status = JobStatus::default();
        self.stems.created.clear();
        self.stems.created_clips.clear();
        if let Some(dialog) = &mut self.stems.dialog {
            dialog.step = DialogStep::Separating;
        }
        self.touch();
        Ok(())
    }

    /// Takes the download's and the separation's news. Returns whether
    /// either is still going, so a window keeps drawing its progress.
    pub(super) fn poll_stems(&mut self) -> bool {
        self.poll_stems_held(self.landings_wait())
    }

    /// [`Shell::poll_stems`], with `held` set while a transaction is open:
    /// the separation's result then stays on its worker, so it doesn't land
    /// inside a drag's undo step.
    fn poll_stems_held(&mut self, held: bool) -> bool {
        // Spec: stem-separation R4. Once the download ends, the dialog
        // offers the stems, or goes back to the package list with the
        // reason it failed.
        if self
            .stems
            .dialog
            .as_ref()
            .is_some_and(|d| d.step == DialogStep::Installing)
            && !self.packages_active(&self.stems_packages())
        {
            let installed = self.stem_packages_installed();
            if let Some(dialog) = &mut self.stems.dialog {
                dialog.step = if installed {
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
            .filter(|_| !held)
            .and_then(|running| running.worker.poll());
        if let Some(ended) = ended {
            let running = self.stems.separation.take().expect("polled above");
            self.stems.provider = running.worker.progress().provider();
            match ended {
                Ended::Done(separated) => {
                    if let Err(error) = self.write_stems(running.args, running.plan, &separated) {
                        self.separation_ended(JobState::Failed, Some(error));
                    }
                }
                Ended::Cancelled => self.separation_ended(JobState::Cancelled, None),
                Ended::Failed(error) => self.separation_ended(JobState::Failed, Some(error)),
            }
        }
        self.packages_busy() || self.stems.separation.is_some()
    }

    /// Cancels the separation under way and forgets the dialog and the
    /// tracks of the last one, for a set that's been replaced.
    pub(super) fn drop_separation(&mut self) {
        if let Some(running) = self.stems.separation.take() {
            running.worker.progress().cancel();
        }
        self.stems.separation_status = JobStatus::default();
        self.stems.writing = false;
        self.stems.provider.clear();
        self.stems.created.clear();
        self.stems.created_clips.clear();
        self.stems.dialog = None;
    }

    fn separation_ended(&mut self, state: JobState, error: Option<String>) {
        self.stems.writing = false;
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
    /// whole (R9 to R11, R28).
    fn write_stems(
        &mut self,
        args: SeparateArgs,
        plan: Plan,
        separated: &[Separated],
    ) -> Result<(), String> {
        let mut items = Vec::new();
        let mut requests: Vec<(usize, usize, Request)> = Vec::new();
        for (piece, separated) in plan.pieces.iter().zip(separated) {
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
            for (index, output) in outputs.iter().enumerate() {
                let parts: Vec<Stem> = output.parts.iter().copied().map(stem_of).collect();
                let frames = crate::stems::file_frames(separated, &parts);
                requests.push((
                    items.len(),
                    index,
                    Request {
                        kind: Kind::Stems,
                        name: format!("{} {}.wav", piece.base, parts_name(&output.parts)),
                        spec: bl_media::EncodeSpec {
                            file_type: bl_media::FileType::Wav,
                            bit_depth: bl_media::BitDepth::Float32,
                            dither: bl_media::Dither::None,
                            sample_rate: bl_analysis::stems::RATE,
                            channels: 2,
                        },
                        frames: (frames.len() / 2) as u64,
                        produce: Box::new(move |sink| sink.write(&frames)),
                    },
                ));
            }
            items.push(Item {
                written: outputs.iter().map(|_| None).collect(),
                outputs,
            });
        }
        let landing = Rc::new(RefCell::new(Landing { plan, items }));
        self.stems.writing = true;
        for (item, index, request) in requests {
            let landing = Rc::clone(&landing);
            self.start_processed(request, move |shell, written| {
                let ready = {
                    let mut landing = landing.borrow_mut();
                    landing.items[item].written[index] =
                        Some((ContentHash(*written.hash.as_bytes()), written.path));
                    landing
                        .items
                        .iter()
                        .all(|item| item.written.iter().all(Option::is_some))
                };
                if ready && let Err(error) = shell.land_stems(&landing.borrow()) {
                    shell.separation_ended(JobState::Failed, Some(error.clone()));
                    return Err(error);
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

    /// The edit: the new tracks with their clips, and the source
    /// deactivated, as one undo step (R9, R11, R23 to R27, R29).
    fn land_stems(&mut self, landing: &Landing) -> Result<(), String> {
        let look = &self.prefs.look;
        let env = Env {
            media: &self.media,
            edge_fades: self.prefs.create_fades_on_clip_edges.0,
            warping: crate::auto_warp::Warping {
                prefs: self.prefs.warp,
                analysis: self.analysis.as_ref(),
            },
        };
        let mut made = Made::default();
        self.editor
            .transact(stems_ids::STEP, |editor| {
                for (piece, item) in landing.plan.pieces.iter().zip(&landing.items) {
                    land_piece(editor, look, &env, landing, piece, item, &mut made)?;
                }
                Ok::<_, String>(())
            })
            .map_err(|e| e.to_string())?;
        self.changed();
        self.stems.created = made.tracks;
        self.stems.created_clips = made.clips;
        self.separation_ended(JobState::Done, None);
        Ok(())
    }

    /// Waits until the download and the separation under way have finished
    /// and the stems' edit has landed, for tests and headless runs.
    pub fn wait_stems(&mut self) {
        loop {
            let downloading = self.poll_packages();
            if !(self.poll_stems_held(false) || downloading || self.is_processing()) {
                break;
            }
            self.land_processed_files();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Makes one piece's tracks and clips, and deactivates its source.
fn land_piece(
    editor: &mut Editor,
    look: &bl_shell_spec::settings::ThemeLook,
    env: &Env<'_>,
    landing: &Landing,
    piece: &Piece,
    item: &Item,
    made: &mut Made,
) -> Result<(), String> {
    let plan = &landing.plan;
    let doc = editor.document();
    // The track the stems follow, whose devices, mixer and automation the
    // group takes (R27), and the colour the group takes (R26).
    let (source_track, group_colour, (parent, below)) = match &plan.source {
        StemSource::Clip { clip } => {
            let track = doc
                .clip_track(*clip)
                .ok_or("the clip's track was deleted")?;
            let colour = doc
                .clip(*clip)
                .and_then(|c| c.colour)
                .or_else(|| doc.track(track).map(|t| t.colour()));
            (Some(track), colour, after(doc, track))
        }
        StemSource::TimeSelection { .. } => {
            let track = piece.track.ok_or("a time selection's piece has a track")?;
            (
                Some(track),
                doc.track(track).map(|t| t.colour()),
                after(doc, track),
            )
        }
        StemSource::File { .. } => (
            None,
            plan.after.and_then(|t| doc.track(t)).map(|t| t.colour()),
            arrange::place_below(doc, plan.after, 0),
        ),
    };
    let parts: Vec<(StemKind, String, ContentHash, &Path)> = item
        .outputs
        .iter()
        .zip(&item.written)
        .filter(|(output, _)| output.track)
        .filter_map(|(output, written)| {
            written.as_ref().map(|(hash, path)| {
                (
                    output.parts[0],
                    parts_name(&output.parts),
                    *hash,
                    path.as_path(),
                )
            })
        })
        .collect();
    // Spec: stem-separation R9. More than one part goes in a new group; one
    // part, or a merge, gets a track of its own there.
    let grouped = parts.len() > 1;
    let (parent, first) = if grouped {
        let group = new_track(
            editor,
            source_track,
            TrackKind::Group,
            &piece.base,
            parent,
            below,
        )?;
        made.tracks.push(group);
        match group_colour {
            Some(colour) => set_colour(editor, group, colour)?,
            None => crate::colours::colour_new(editor, look, &[group], &[])
                .map_err(|e| e.to_string())?,
        }
        (Some(group), 0)
    } else {
        (parent, below)
    };
    for (offset, (kind, name, hash, path)) in parts.iter().enumerate() {
        // The one track of a separation with no group holds the copy of the
        // source track's devices itself.
        let track = new_track(
            editor,
            if grouped { None } else { source_track },
            TrackKind::Audio,
            name,
            parent,
            first + offset,
        )?;
        made.tracks.push(track);
        // Spec: stem-separation R26. Each part's own colour.
        set_colour(editor, track, kind.swatch())?;
        let clip = new_clip(editor, env, plan, track, name, *hash, path)?;
        made.clips.push(clip);
        editor
            .apply(Command::SetClipColour {
                clips: vec![clip],
                colour: kind.swatch(),
            })
            .map_err(|e| e.to_string())?;
    }
    // Spec: stem-separation R11, R25. The source is deactivated: the clip,
    // or the parts of the track's clips inside the selection.
    match &plan.source {
        StemSource::Clip { clip } => {
            editor
                .apply(Command::SetClipMute {
                    clip: *clip,
                    mute: true,
                })
                .map_err(|e| e.to_string())?;
        }
        StemSource::TimeSelection { start, end, .. } => {
            let track = piece.track.expect("checked above");
            deactivate_span(editor, track, *start, *end)?;
        }
        StemSource::File { .. } => {}
    }
    Ok(())
}

/// A new track at `index` in `parent`: a copy of `copy_of`'s devices, mixer
/// settings and Arrangement automation without its clips (R27) when it's
/// given, an empty one otherwise.
fn new_track(
    editor: &mut Editor,
    copy_of: Option<TrackId>,
    kind: TrackKind,
    name: &str,
    parent: Option<TrackId>,
    index: usize,
) -> Result<TrackId, String> {
    let Some(source) = copy_of.and_then(|t| crate::tracks::snapshot(editor.document(), t)) else {
        let id: TrackId = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .map_err(|e| e.to_string())?;
        return Ok(id);
    };
    let mut data = source;
    data.kind = kind;
    data.name = name.to_owned();
    data.mixer.arm = false;
    data.clips.clear();
    data.slots.clear();
    data.removed_stop_buttons.clear();
    data.take_lanes.clear();
    data.audition = None;
    data.frozen = None;
    data.children.clear();
    data.info_text.clear();
    let place = crate::tracks::Place {
        parent,
        index,
        returns: 0,
    };
    crate::tracks::copy_tracks_in(editor, &[data], place, None)
        .map_err(|e| e.to_string())?
        .first()
        .copied()
        .ok_or_else(|| "the track wasn't copied".to_owned())
}

fn set_colour(editor: &mut Editor, track: TrackId, colour: u8) -> Result<(), String> {
    editor
        .apply(Command::SetTrackColour {
            tracks: vec![track],
            colour,
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Makes a part's clip on its `track` by the source's kind: a copy of the
/// Arrangement clip (R11), a copy in the Session clip's scene (R23), a new
/// clip of the file at the insert marker (R24), or one spanning the time
/// selection (R25).
fn new_clip(
    editor: &mut Editor,
    env: &Env<'_>,
    plan: &Plan,
    track: TrackId,
    name: &str,
    hash: ContentHash,
    path: &Path,
) -> Result<ClipId, String> {
    let id: ClipId = editor.new_id();
    let doc = editor.document();
    let error = |e: bl_model::EditError| e.to_string();
    match &plan.source {
        StemSource::Clip { clip } => {
            let source = doc.clip(*clip).cloned().ok_or("the clip was deleted")?;
            let scene = doc
                .clip_track(*clip)
                .and_then(|t| doc.track(t))
                .and_then(|t| {
                    t.slots()
                        .find(|(_, slot)| slot.id == *clip)
                        .map(|(scene, _)| scene)
                });
            // A copy of the clip, playing its part: the markers, warping,
            // gain and envelopes carry over, since the file lines up with the
            // clip's, and a Session clip's launch settings with them.
            let mut copy = source;
            copy.id = id;
            copy.name = name.to_owned();
            copy.mute = false;
            let scale = copy.scale;
            if let ClipData::Audio(audio) = &mut copy.data {
                audio.media = hash;
                audio.video = false;
            }
            editor
                .apply(match scene {
                    Some(scene) => Command::CreateSlotClip {
                        track,
                        scene,
                        clip: copy,
                    },
                    None => Command::CreateClip { track, clip: copy },
                })
                .map_err(error)?;
            crate::tracks::keep_scale(editor, id, scale).map_err(error)?;
            Ok(id)
        }
        StemSource::File { .. } => {
            let args = AddToArrangementArgs {
                paths: vec![path.to_string_lossy().into_owned()],
                track: Some(track),
                at: Some(plan.insert),
                spread: false,
                lane: None,
            };
            let (result, _) = arrange::add_files(
                editor,
                env.media,
                plan.insert,
                env.edge_fades,
                env.warping,
                &args,
            )
            .map_err(|e| match e {
                arrange::AddError::InvalidArgs(m) | arrange::AddError::Failed(m) => m,
            })?;
            let clip = result.clips.first().ok_or("the file made no clip")?.clip;
            editor
                .apply(Command::RenameClip {
                    clip,
                    name: name.to_owned(),
                })
                .map_err(error)?;
            Ok(clip)
        }
        StemSource::TimeSelection { start, end, .. } => {
            let tempo = doc.settings().tempo.bpm_at(*end).get();
            let markers = beat_markers(doc, *start, *end);
            let mut clip = Clip::new(
                id,
                *start,
                *end - *start,
                ClipData::Audio(consolidated_audio(hash, 1.0, markers, tempo)),
            );
            clip.name = name.to_owned();
            editor
                .apply(Command::CreateClip { track, clip })
                .map_err(error)?;
            Ok(id)
        }
    }
}

/// Splits the clips of `track` that play in `start..end` at its edges and
/// deactivates their parts inside, so the track plays nothing there (R25).
/// A split gives both pieces the whole clip's fades, so the piece before
/// the range loses its fade-out and the piece after it its fade-in: the
/// clips outside the range play what they played before the split.
fn deactivate_span(
    editor: &mut Editor,
    track: TrackId,
    start: Ticks,
    end: Ticks,
) -> Result<(), String> {
    let error = |e: bl_model::EditError| e.to_string();
    for clip in playing_in(editor.document(), track, start, end) {
        let Some(held) = editor.document().clip(clip).cloned() else {
            continue;
        };
        let mut inside = clip;
        if held.start < start {
            inside = placing::split_off(editor, clip, start).map_err(error)?;
            cut_fade(editor, clip, false)?;
        }
        let inside_end = editor
            .document()
            .clip(inside)
            .map_or(held.end(), |clip| clip.end());
        if end < inside_end {
            let after = placing::split_off(editor, inside, end).map_err(error)?;
            cut_fade(editor, after, true)?;
        }
        editor
            .apply(Command::SetClipMute {
                clip: inside,
                mute: true,
            })
            .map_err(error)?;
    }
    Ok(())
}

/// Takes the fade a split gave `clip` at its cut: its fade-in when `start`
/// is set, its fade-out otherwise.
fn cut_fade(editor: &mut Editor, clip: ClipId, start: bool) -> Result<(), String> {
    let Some(ClipData::Audio(audio)) = editor.document().clip(clip).map(|c| c.data.clone()) else {
        return Ok(());
    };
    editor
        .apply(Command::SetClipFades {
            clip,
            fade_in: if start { 0.0 } else { audio.fade_in },
            fade_out: if start { audio.fade_out } else { 0.0 },
            fade_in_curve: None,
            fade_out_curve: None,
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GesturePhase;
    use bl_analysis::stems::Stems;
    use serde_json::json;

    /// A shell with a short audio clip on a track, and a separation of it
    /// under way whose worker finishes once `release` is sent.
    fn separating(name: &str) -> (Shell, ClipId, std::sync::mpsc::Sender<()>) {
        let file =
            std::env::temp_dir().join(format!("bl-stems-unit-{name}-{}.wav", std::process::id()));
        let mut writer = bl_media::AudioWriter::create(
            &file,
            bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: 48_000,
                channels: 2,
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
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let worker = Worker::start("bl-test-stems", move |_| {
            let _ = wait.recv();
            Ended::Done(vec![Separated {
                from: 0,
                stems: Stems {
                    stems: std::array::from_fn(|_| [vec![0.1; 4_800], vec![0.1; 4_800]]),
                },
            }])
        })
        .unwrap();
        let args = SeparateArgs {
            clip: Some(clip),
            source: None,
            stems: StemKind::ALL.to_vec(),
            quality: StemQuality::default(),
            merge: false,
        };
        let plan = Plan {
            source: StemSource::Clip { clip },
            insert: Ticks::ZERO,
            after: None,
            pieces: vec![Piece {
                track: None,
                base: "Unit".to_owned(),
            }],
        };
        shell.stems.separation = Some(Running { worker, args, plan });
        (shell, clip, release)
    }

    /// Polls until the separation and the files it wrote have landed.
    fn poll_until_landed(shell: &mut Shell) {
        for _ in 0..5_000 {
            shell.poll();
            if shell.stems.separation.is_none() && !shell.is_processing() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("the separation never landed");
    }

    // Spec: stem-separation R11. A separation that finishes while the person
    // drags a clip lands when the drag ends, not inside the drag's
    // transaction, where the drag's next frame would throw it away.
    #[test]
    fn stems_that_finish_during_a_drag_land_after_it() {
        let (mut shell, clip, release) = separating("drag");
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
            shell.stems.separation.is_some(),
            "the result waits for the drag"
        );
        shell
            .gesture(GesturePhase::Update, Some(&frame(1_920)))
            .unwrap();
        shell
            .gesture(GesturePhase::Commit, Some(&frame(1_920)))
            .unwrap();
        poll_until_landed(&mut shell);

        let document = shell.editor().document();
        assert!(!shell.stems.created.is_empty());
        for track in &shell.stems.created {
            assert!(document.track(*track).is_some(), "stem track {track:?}");
        }
        assert!(document.clip(clip).unwrap().mute, "the source is muted");
    }

    // Spec: stem-separation R1. A separation under way belongs to the set
    // it began in: a new set gets neither its tracks nor its muted clip.
    #[test]
    fn a_new_set_drops_the_separation_under_way() {
        let (mut shell, _clip, release) = separating("replaced");
        shell.run("set.new", json!({})).unwrap();
        let fresh = shell.editor().document().clone();
        assert!(shell.stems.separation.is_none());
        let _ = release.send(());
        for _ in 0..2_000 {
            shell.poll();
            std::thread::yield_now();
        }
        assert_eq!(*shell.editor().document(), fresh);
        assert!(shell.stems.created.is_empty());
        assert!(!shell.is_processing());
    }
}

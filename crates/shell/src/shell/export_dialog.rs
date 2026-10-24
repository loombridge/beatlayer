//! The Export Audio/Video dialog and the export it starts, through the
//! shell (WP-6.71; the export spec, R28 to R45): the `export.*` commands
//! that open, change and close the dialog, start an export and stop it,
//! the export on its worker thread, and what lands when it ends.
//!
//! An export renders a snapshot of the set, so the main window goes on
//! drawing. A set that needs the main thread (a plug-in device, whose
//! plug-in can only be lent there, or external hardware, which renders in
//! real time on the audio device) renders on it instead, as before the
//! dialog.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bl_core::time::Ticks;
use bl_shell_spec::export::{
    self as export_ids, ChoicesPatch, ExportArgs, ExportChoices, ExportDialogStatus, ExportResult,
    ExportStatus, FileType, RenderedTrack, RunningStatus, SAMPLE_RATES, StartArgs,
};
use serde::{Deserialize, Serialize};

use super::*;
use crate::export::{ExportError, range};

/// What Export keeps between starts, as a preference (the export spec,
/// R39): the dialog's choices apart from Render Start and Render Length,
/// and the folder of the last export (R40).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    /// The dialog's controls.
    pub choices: ExportChoices,
    /// The folder the last export went to.
    pub folder: Option<PathBuf>,
}

/// The open dialog.
struct Dialog {
    choices: ExportChoices,
    start: Ticks,
    length: Ticks,
    /// The save dialog is open: the Export dialog waits behind it, and
    /// comes back with its choices if the save dialog is cancelled (R40).
    picking: bool,
}

/// How an export ended: its result, and the files whose peaks and analysis
/// to keep (Create Analysis File, R34).
struct Finished {
    result: Result<ExportResult, ShellError>,
    analyse: Vec<bl_media::ContentHash>,
}

/// An export on its thread.
struct Running {
    finished: Receiver<Finished>,
    /// The files it writes and the share rendered, from 0 to 1.
    shared: Arc<Mutex<(Vec<PathBuf>, f64)>>,
}

/// The dialog and the export in the shell.
#[derive(Default)]
pub(super) struct ExportState {
    dialog: Option<Dialog>,
    running: Option<Running>,
    /// How the last export ended, until a caller of `export.audio` takes it.
    ended: Option<Result<ExportResult, ShellError>>,
    /// A caller of `export.audio` waits for this export's result, so no
    /// other export starts until it has taken it.
    awaited: bool,
}

impl Shell {
    /// The Export Audio/Video dialog, if it's open and not behind the
    /// save dialog (R28).
    #[must_use]
    pub fn export_dialog(&self) -> Option<ExportDialogStatus> {
        let dialog = self.export.dialog.as_ref().filter(|d| !d.picking)?;
        Some(self.export_dialog_status(dialog))
    }

    /// What `export.status` reports (R44).
    #[must_use]
    pub fn export_status(&self) -> ExportStatus {
        ExportStatus {
            dialog: self
                .export
                .dialog
                .as_ref()
                .map(|dialog| self.export_dialog_status(dialog)),
            export: self.export.running.as_ref().map(|running| {
                let (files, progress) = running
                    .shared
                    .lock()
                    .map_or_else(|e| e.into_inner().clone(), |s| s.clone());
                RunningStatus {
                    files: files.iter().map(|f| f.display().to_string()).collect(),
                    progress,
                }
            }),
        }
    }

    fn export_dialog_status(&self, dialog: &Dialog) -> ExportDialogStatus {
        let rate = self.engine_rate();
        let problems = dialog.choices.problems(
            rate,
            self.arrangement.selected_tracks().len(),
            dialog.length,
        );
        let mut choices = dialog.choices.clone();
        choices.sample_rate = Some(choices.rate(rate));
        ExportDialogStatus {
            choices,
            start: dialog.start,
            length: dialog.length,
            can_export: problems.blocked.is_none() && self.export.running.is_none(),
            reason: problems.blocked.or_else(|| {
                self.export
                    .running
                    .is_some()
                    .then(|| "An export is under way.".to_owned())
            }),
            disabled: problems.disabled,
        }
    }

    /// Render Start and Render Length when the dialog opens: the
    /// Arrangement's time selection, else the whole set (R31, R1); zero
    /// length for a set with nothing to export.
    #[must_use]
    pub fn export_default_range(&self) -> (Ticks, Ticks) {
        match range(
            self.editor.document(),
            None,
            None,
            self.arrangement.time_selection(),
        ) {
            Ok((from, to, _)) => (from, to - from),
            Err(_) => (Ticks::ZERO, Ticks::ZERO),
        }
    }

    /// The folder the save dialog opens in: the last export's, else the
    /// set's project folder (R40).
    #[must_use]
    pub fn export_dialog_folder(&self) -> Option<PathBuf> {
        self.prefs
            .export
            .folder
            .clone()
            .filter(|folder| folder.is_dir())
            .or_else(|| self.save_dialog_folder())
    }

    /// The files that exist already among those the dialog's export would
    /// write, apart from `path` itself, which the save dialog asked about
    /// (R40).
    #[must_use]
    pub fn export_conflicts(&self, path: &Path) -> Vec<PathBuf> {
        let Some(dialog) = &self.export.dialog else {
            return Vec::new();
        };
        let args = self.export_args(dialog, &path.to_string_lossy(), true);
        crate::export::planned_files(
            self.editor.document(),
            self.arrangement.time_selection(),
            self.arrangement.selected_tracks(),
            self.engine_rate(),
            &args,
        )
        .unwrap_or_default()
        .into_iter()
        .filter(|file| file != path && file.exists())
        .collect()
    }

    fn export_args(
        &self,
        dialog: &Dialog,
        path: &str,
        replace: bool,
    ) -> bl_shell_spec::export::ExportArgs {
        dialog.choices.args(
            path,
            dialog.start,
            dialog.length,
            self.engine_rate(),
            replace,
        )
    }

    /// Whether an `export.*` command can run now.
    pub(super) fn can_export(&self, id: &str) -> bool {
        match id {
            export_ids::CLOSE_DIALOG | export_ids::SET_CHOICES => self.export.dialog.is_some(),
            export_ids::START => self
                .export_dialog_status_if_open()
                .is_some_and(|status| status.can_export),
            export_ids::CANCEL => self.export.running.is_some(),
            _ => true,
        }
    }

    fn export_dialog_status_if_open(&self) -> Option<ExportDialogStatus> {
        self.export
            .dialog
            .as_ref()
            .map(|dialog| self.export_dialog_status(dialog))
    }

    /// Runs the dialog's commands (R28 to R45).
    pub(super) fn run_export_dialog(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        // A shell nobody polls (`bl-cli mcp`) learns that the export ended
        // when the next command asks.
        self.poll_export();
        let invalid = |message: String| ShellError::InvalidArgs {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            export_ids::OPEN_DIALOG => {
                Self::args::<NoArgs>(id, args)?;
                if let Some(dialog) = &mut self.export.dialog {
                    // After a cancelled save dialog: the dialog comes back
                    // with its choices (R40).
                    dialog.picking = false;
                } else {
                    let mut choices = self.prefs.export.choices.clone();
                    // R39: a single track chosen before that no longer
                    // exists falls back to Main.
                    if let RenderedTrack::Track(track) = choices.rendered_track
                        && self.editor.document().track(track).is_none()
                    {
                        choices.rendered_track = RenderedTrack::Main;
                    }
                    choices.settle();
                    let (start, length) = self.export_default_range();
                    self.export.dialog = Some(Dialog {
                        choices,
                        start,
                        length,
                        picking: false,
                    });
                }
            }
            export_ids::CLOSE_DIALOG => {
                Self::args::<NoArgs>(id, args)?;
                self.export.dialog = None;
            }
            export_ids::SET_CHOICES => {
                let patch: ChoicesPatch = Self::args(id, args)?;
                let signatures = self.editor.document().settings().time_signatures.clone();
                let tracks = self.editor.document();
                let Some(dialog) = &mut self.export.dialog else {
                    return Err(unavailable(
                        "the Export dialog isn't open: run `export.open_dialog` first",
                    ));
                };
                let mut choices = dialog.choices.clone();
                let ChoicesPatch {
                    rendered_track,
                    include_return_and_main_effects,
                    render_as_loop,
                    convert_to_mono,
                    normalize,
                    create_analysis_file,
                    sample_rate,
                    encode_pcm,
                    file_type,
                    bit_depth,
                    dither,
                    encode_mp3,
                    start,
                    length,
                } = patch;
                if let Some(track) = rendered_track {
                    // R30: Main, All Individual Tracks, Selected Tracks
                    // Only, or an audio, MIDI, group or return track.
                    if let RenderedTrack::Track(id) = track
                        && tracks
                            .track(id)
                            .is_none_or(|t| t.kind() == bl_model::TrackKind::Main)
                    {
                        return Err(invalid(format!(
                            "no audio, MIDI, group or return track has the ID {}",
                            id.0
                        )));
                    }
                    choices.rendered_track = track;
                }
                if let Some(rate) = sample_rate {
                    if !SAMPLE_RATES.contains(&rate) {
                        return Err(invalid(format!(
                            "{rate} Hz isn't in the Sample Rate list: 22050, 32000, 44100, 48000, \
                             88200, 96000, 176400 or 192000"
                        )));
                    }
                    choices.sample_rate = Some(rate);
                }
                if let Some(bits) = bit_depth {
                    if ![16, 24, 32].contains(&bits) {
                        return Err(invalid(format!("{bits} bits; give 16, 24 or 32")));
                    }
                    choices.bit_depth = bits;
                }
                if let Some(file_type) = file_type {
                    if file_type == FileType::Mp3 {
                        return Err(invalid(
                            "File Type is WAV, AIFF or FLAC; turn on Encode MP3 for an MP3"
                                .to_owned(),
                        ));
                    }
                    choices.file_type = file_type;
                }
                choices.include_return_and_main_effects = include_return_and_main_effects
                    .unwrap_or(choices.include_return_and_main_effects);
                choices.render_as_loop = render_as_loop.unwrap_or(choices.render_as_loop);
                choices.convert_to_mono = convert_to_mono.unwrap_or(choices.convert_to_mono);
                choices.normalize = normalize.unwrap_or(choices.normalize);
                choices.create_analysis_file =
                    create_analysis_file.unwrap_or(choices.create_analysis_file);
                choices.encode_pcm = encode_pcm.unwrap_or(choices.encode_pcm);
                choices.dither = dither.unwrap_or(choices.dither);
                choices.encode_mp3 = encode_mp3.unwrap_or(choices.encode_mp3);
                // R36: choosing FLAC with 32 chosen sets 24.
                choices.settle();
                dialog.choices = choices;
                if let Some(start) = start {
                    dialog.start = start
                        .resolve(&signatures)
                        .map_err(invalid)?
                        .max(Ticks::ZERO);
                }
                if let Some(length) = length {
                    dialog.length = length.max(Ticks::ZERO);
                }
            }
            export_ids::START => {
                let StartArgs { path, replace } = Self::args(id, args)?;
                self.start_from_dialog(id, path, replace)?;
            }
            export_ids::STATUS => {
                Self::args::<NoArgs>(id, args)?;
            }
            export_ids::CANCEL => {
                Self::args::<NoArgs>(id, args)?;
                self.realtime.cancel();
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        self.touch();
        Ok(to_value(&self.export_status()))
    }

    /// Export, in the dialog (R40): without a path the save dialog is asked
    /// for, and with one the export starts.
    fn start_from_dialog(
        &mut self,
        id: &str,
        path: Option<String>,
        replace: bool,
    ) -> Result<(), ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let Some(status) = self.export_dialog_status_if_open() else {
            return Err(unavailable(
                "the Export dialog isn't open: run `export.open_dialog` first".to_owned(),
            ));
        };
        if !status.can_export {
            return Err(unavailable(
                status
                    .reason
                    .unwrap_or_else(|| "Export is disabled".to_owned()),
            ));
        }
        // R39: Export keeps the choices.
        // The rate stays the engine's until a rate is chosen (R35).
        self.prefs.export.choices = self
            .export
            .dialog
            .as_ref()
            .expect("checked above")
            .choices
            .clone();
        self.save_preferences();
        let dialog = self.export.dialog.as_mut().expect("checked above");
        let Some(path) = path else {
            dialog.picking = true;
            let name = format!(
                "{}.{}",
                bl_project::preset::file_safe(&self.set_name()),
                self.prefs.export.choices.extension()
            );
            self.effects.push(Effect::PickExportPath {
                file_name: name,
                folder: self.export_dialog_folder(),
            });
            return Ok(());
        };
        let dialog = self.export.dialog.take().expect("checked above");
        let args = self.export_args(&dialog, &path, replace);
        self.begin_export(args)
    }

    /// Starts an export on its worker thread, from a snapshot of the set,
    /// and returns at once (R42). A set that needs the main thread renders
    /// here instead, and has ended when this returns.
    ///
    /// # Errors
    ///
    /// [`ShellError`] if an export is under way already, or the thread
    /// can't start.
    pub fn begin_export(&mut self, args: ExportArgs) -> Result<(), ShellError> {
        self.poll_export();
        if self.export.awaited && self.export.ended.is_some() {
            return Err(ShellError::Unavailable {
                command: export_ids::AUDIO.to_owned(),
                reason: "an export has just ended and its caller hasn't had its result yet"
                    .to_owned(),
            });
        }
        if let Some(running) = &self.export.running {
            let files = running
                .shared
                .lock()
                .map(|s| s.0.clone())
                .unwrap_or_default();
            let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
            return Err(ShellError::Unavailable {
                command: export_ids::AUDIO.to_owned(),
                reason: format!("an export is under way: {}", names.join(", ")),
            });
        }
        // Spec: offline-plugin-renders R7. Export stops the song first.
        if self.audio.transport_state().playing {
            let _ = self.audio.send_transport(bl_engine::TransportCommand::Stop);
        }
        let engine_rate = self.engine_rate();
        let selection = self.arrangement.time_selection();
        let selected: Vec<TrackId> = self.arrangement.selected_tracks().to_vec();
        let document = self.editor.document().clone();
        let media = self.engine_media();
        let store = Arc::clone(&self.media);
        let session = self.realtime.clone();
        // A render that ran before leaves nothing behind: the session
        // forgets its cancel, its skip and its restart.
        session.reset();
        let shared = Arc::new(Mutex::new((Vec::new(), 0.0)));
        let progress_shared = Arc::clone(&shared);
        let mut progress = move |files: &[PathBuf], share: f64| {
            if let Ok(mut shared) = progress_shared.lock() {
                if shared.0.len() != files.len() {
                    shared.0 = files.to_vec();
                }
                shared.1 = share;
            }
        };
        let analyse = args.create_analysis_file;
        let finish = move |result: Result<ExportResult, ShellError>| {
            // Spec: export R34. Each written file is known to the media
            // store by its content hash, which its peaks and analysis are
            // kept under.
            let analyse = match &result {
                Ok(result) if analyse => result
                    .files
                    .iter()
                    .filter_map(|file| store.import(file).ok())
                    .collect(),
                _ => Vec::new(),
            };
            Finished { result, analyse }
        };
        let (send, finished) = mpsc::channel();
        // A plug-in can only be lent on the main thread, and external
        // hardware renders on the audio device.
        let needs_main =
            crate::plugins::has_plugin(&document) || !document.external_devices().is_empty();
        if needs_main {
            let hosts = bl_plugin_host::Hosts {
                clap: Some(&self.plugins),
                vst3: Some(&self.vst3),
                lv2: Some(&self.lv2),
                au: Some(&self.au),
            };
            let result = crate::export::run_command(
                &document,
                media,
                hosts,
                selection,
                &selected,
                engine_rate,
                &args,
                Some(&mut self.audio),
                &session,
                &mut progress,
            );
            let _ = send.send(finish(result));
        } else {
            std::thread::Builder::new()
                .name("bl-shell-export".to_owned())
                .spawn(move || {
                    let result = crate::export::run_command(
                        &document,
                        media,
                        bl_plugin_host::Hosts::default(),
                        selection,
                        &selected,
                        engine_rate,
                        &args,
                        None,
                        &session,
                        &mut progress,
                    );
                    let _ = send.send(finish(result));
                })
                .map_err(|e| ShellError::Failed {
                    command: export_ids::AUDIO.to_owned(),
                    message: format!("couldn't start the export: {e}"),
                })?;
        }
        self.export.running = Some(Running { finished, shared });
        self.export.ended = None;
        self.export.awaited = false;
        self.touch();
        Ok(())
    }

    /// Takes the export's news: when it has ended, the Status Bar says where
    /// the file went, or why it failed (R41), the folder is kept (R40), and
    /// the written files' peaks and analysis are started if asked for
    /// (R34). Returns whether an export is still going, so the progress
    /// window keeps drawing.
    pub(super) fn poll_export(&mut self) -> bool {
        let Some(running) = &self.export.running else {
            return false;
        };
        let Finished { result, analyse } = match running.finished.try_recv() {
            Ok(finished) => finished,
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => Finished {
                result: Err(ShellError::Failed {
                    command: export_ids::AUDIO.to_owned(),
                    message: "the export stopped unexpectedly".to_owned(),
                }),
                analyse: Vec::new(),
            },
        };
        self.export.running = None;
        match &result {
            Ok(done) => {
                let name = Path::new(&done.path)
                    .file_name()
                    .map_or_else(|| done.path.clone(), |n| n.to_string_lossy().into_owned());
                let mut text = if done.files.len() > 1 {
                    format!("Exported {} files, beginning with {name}", done.files.len())
                } else {
                    format!("Exported {name}")
                };
                if !done.missing_media.is_empty() {
                    text.push_str(&format!(
                        "; {} media file{} missing and played silence",
                        done.missing_media.len(),
                        if done.missing_media.len() == 1 {
                            " was"
                        } else {
                            "s were"
                        }
                    ));
                }
                self.set_message(text, false);
                self.prefs.export.folder = Path::new(&done.path).parent().map(Path::to_path_buf);
            }
            Err(error) => {
                let cancelled = matches!(error, ShellError::Failed { message, .. }
                    if *message == ExportError::Cancelled.to_string());
                self.set_message(
                    if cancelled {
                        "The export was cancelled.".to_owned()
                    } else {
                        error.to_string()
                    },
                    !cancelled,
                );
            }
        }
        for hash in analyse {
            if let Some(cache) = self.analysis_cache() {
                cache.request(hash);
            }
            if let Some(cache) = self.peak_cache() {
                cache.request(hash);
            }
        }
        self.export.ended = Some(result);
        self.touch();
        false
    }

    /// How the last export ended, once: for the caller that waits for
    /// `export.audio`.
    pub fn take_export_result(&mut self) -> Option<Result<ExportResult, ShellError>> {
        let ended = self.export.ended.take();
        if ended.is_some() {
            self.export.awaited = false;
        }
        ended
    }

    /// Whether a caller of `export.audio` is waiting for an export's result.
    #[must_use]
    pub fn export_awaited(&self) -> bool {
        self.export.awaited
    }

    /// Cancels the export under way, if any, and waits until its thread has
    /// removed what it wrote: for quitting.
    pub fn stop_export(&mut self) {
        if let Some(running) = self.export.running.take() {
            self.realtime.cancel();
            let _ = running.finished.recv();
        }
    }

    /// Whether an export is under way.
    #[must_use]
    pub fn export_running(&self) -> bool {
        self.export.running.is_some()
    }

    /// Starts `export.audio` on its thread and returns at once, for a caller
    /// that goes on drawing until [`Shell::take_export_result`] has the
    /// result (R44).
    ///
    /// # Errors
    ///
    /// [`ShellError`] for arguments that don't fit the command, or if an
    /// export is under way.
    pub fn start_export(&mut self, args: Value) -> Result<(), ShellError> {
        let args: ExportArgs = Self::args(export_ids::AUDIO, args)?;
        self.begin_export(args)?;
        self.export.awaited = true;
        Ok(())
    }

    /// Runs `export.audio`: starts the export and waits for it to end, then
    /// gives its result (R44).
    pub(super) fn run_export_audio(&mut self, args: Value) -> Result<Value, ShellError> {
        self.start_export(args)?;
        while self.poll_export() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.take_export_result()
            .expect("an export that ended has a result")
            .map(|result| to_value(&result))
    }

    /// Where the written files' peaks are kept: in memory only until the
    /// app gives the platform's cache folder, so a headless shell writes no
    /// files.
    pub fn use_peak_options(&mut self, options: bl_media::PeakCacheOptions) {
        self.peak_options = options;
        self.peaks = None;
    }

    /// Keeps the written files' peaks in the platform's cache folder, where
    /// the Arrangement's waveforms look for them, as the app does.
    pub fn use_platform_peak_cache(&mut self) {
        self.use_peak_options(bl_media::PeakCacheOptions::platform_default());
    }

    fn peak_cache(&mut self) -> Option<&bl_media::PeakCache> {
        if self.peaks.is_none() {
            self.peaks =
                bl_media::PeakCache::new(Arc::clone(&self.media), self.peak_options.clone()).ok();
        }
        self.peaks.as_ref()
    }

    /// The file's peaks, waiting until they are ready. For tools and tests;
    /// the app never waits.
    ///
    /// # Errors
    ///
    /// The build's error.
    pub fn wait_peaks(
        &mut self,
        media: bl_model::ContentHash,
    ) -> Result<Arc<bl_media::Peaks>, Arc<bl_media::MediaError>> {
        let hash = bl_media::ContentHash::from_bytes(media.0);
        match self.peak_cache() {
            Some(cache) => cache.wait(hash),
            None => Err(Arc::new(bl_media::MediaError::UnknownMedia(hash))),
        }
    }
}

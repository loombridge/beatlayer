//! The autosampler at work (WP-5.16, IMP-27; the multisampler spec, R64 to
//! R73): rendering the instrument alone, writing the samples, and making
//! the multisampler they become.
//!
//! The run is a job the shell keeps: it makes one sample at a time, so the
//! progress and the note it is on are known, and Cancel can stop it between
//! two samples. The agent's command waits for the job; the dialog reads the
//! progress while it runs.

use std::path::PathBuf;
use std::time::Instant;

use bl_core::time::Ticks;
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{
    Command, ContentHash, DeviceId, DeviceRole, Document, Layer, LayerId, ReleaseMode, SustainMode,
    TrackId, TrackKind,
};
use bl_shell_spec::autosampler as ids;

use super::*;
use crate::autosample::{self, Plan, Planned, Settings};
use crate::processed::Kind;

/// The crossfade a loop gets, in milliseconds (R71).
const LOOP_CROSSFADE_MS: f64 = 10.0;

/// What the autosampler is doing now (R73).
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Whether a sample is running.
    pub running: bool,
    /// How many samples are made.
    pub done: usize,
    /// How many samples there are.
    pub total: usize,
    /// The note it is on.
    pub note: Option<u8>,
    /// The velocity it plays at.
    pub velocity: Option<u8>,
    /// How long it has taken, in seconds.
    pub took: f64,
}

/// The autosampler's dialog, open for an instrument (R64, R65).
#[derive(Clone, Debug)]
pub struct Dialog {
    /// The instrument to sample.
    pub device: DeviceId,
    /// The settings the dialog holds.
    pub settings: Settings,
}

/// The autosampler at work: the samples it makes and the files they are in.
pub struct Job {
    /// The instrument sampled.
    device: DeviceId,
    /// The track it is on.
    track: TrackId,
    /// The settings.
    settings: Settings,
    /// The folder this run writes into, free when it started (R69).
    folder: PathBuf,
    /// The plan.
    plan: Plan,
    /// The sample being made.
    index: usize,
    /// The files written so far, deleted if the run is stopped.
    written: Vec<PathBuf>,
    /// The media the samples joined, added to the set when it lands.
    media: Vec<(ContentHash, PathBuf, u64)>,
    /// The layers made so far.
    layers: Vec<Layer>,
    /// The samples left without a loop, named.
    without_loops: Vec<String>,
    /// The samples made, with their files and loops.
    files: Vec<ids::AutosampleFile>,
    /// When it started.
    started: Instant,
}

impl Shell {
    /// Whether the autosampler's commands can run.
    pub(super) fn can_autosampler(&self, id: &str, _args: &Value) -> bool {
        match id {
            ids::AUTOSAMPLE_CANCEL => self.autosample.is_some() || self.autosample_dialog.is_some(),
            _ => self.autosample.is_none(),
        }
    }

    /// Runs one of the autosampler's commands.
    pub(super) fn run_autosampler(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::AUTOSAMPLE_OPEN => {
                let args: ids::AutosampleOpenArgs = Self::args(id, args)?;
                self.open_autosampler(id, args)
            }
            ids::AUTOSAMPLE => {
                let args: ids::AutosampleArgs = Self::args(id, args)?;
                self.autosample(id, args)
            }
            ids::AUTOSAMPLE_CANCEL => {
                Self::args::<NoArgs>(id, args)?;
                let cancelled = self.stop_autosample();
                self.autosample_dialog = None;
                Ok(to_value(&ids::AutosampleCancelled { cancelled }))
            }
            _ => Err(self.failed(id, "unknown command")),
        }
    }

    /// Opens the autosampler's dialog for an instrument (R64).
    fn open_autosampler(
        &mut self,
        id: &str,
        args: ids::AutosampleOpenArgs,
    ) -> Result<Value, ShellError> {
        let device = match args.device.or_else(|| self.devices.selected()) {
            Some(device) => device,
            None => return Err(self.failed(id, "no instrument is selected")),
        };
        let source = self
            .editor
            .document()
            .device(device)
            .cloned()
            .ok_or_else(|| self.failed(id, "the instrument is gone"))?;
        if source.role != DeviceRole::Instrument {
            return Err(self.failed(id, "the device isn't an instrument"));
        }
        let settings = Settings {
            name: source.name.clone(),
            ..Settings::default()
        };
        self.autosample_dialog = Some(Dialog { device, settings });
        Ok(to_value(&ids::AutosamplerOpened { device }))
    }

    /// What the autosampler is doing now (R73).
    #[must_use]
    pub fn autosample_status(&self) -> Status {
        match &self.autosample {
            Some(job) => {
                let sample = job.plan.samples.get(job.index);
                Status {
                    running: true,
                    done: job.index,
                    total: job.plan.samples.len(),
                    note: sample.map(|sample| sample.note),
                    velocity: sample.map(|sample| sample.velocity),
                    took: job.started.elapsed().as_secs_f64(),
                }
            }
            None => Status::default(),
        }
    }

    /// The autosampler's dialog, open for an instrument (R64).
    #[must_use]
    pub fn autosampler_dialog(&self) -> Option<&Dialog> {
        self.autosample_dialog.as_ref()
    }

    /// What the last autosample made (R73).
    #[must_use]
    pub fn autosample_result(&self) -> Option<&ids::Autosampled> {
        self.autosample_result_value.as_ref()
    }

    /// Starts the autosample (R64 to R69).
    fn autosample(&mut self, id: &str, args: ids::AutosampleArgs) -> Result<Value, ShellError> {
        if self.autosample.is_some() {
            return Err(self.failed(id, "an autosample is already running"));
        }
        let device = match args.device.or_else(|| self.devices.selected()) {
            Some(device) => device,
            None => return Err(self.failed(id, "no instrument is selected")),
        };
        let source = self
            .editor
            .document()
            .device(device)
            .cloned()
            .ok_or_else(|| self.failed(id, "the instrument is gone"))?;
        if source.role != DeviceRole::Instrument {
            return Err(self.failed(id, "the device isn't an instrument"));
        }
        // Spec: multisampler R68. The external instrument's real-time route
        // isn't built yet, and the message says so.
        if source.external.is_some() {
            return Err(self.failed(
                id,
                "the external instrument isn't sampled yet: it needs the real-time route",
            ));
        }
        // Spec: multisampler R69. The samples are written into the project's
        // folder, as the processed files are, and a set never saved has none.
        if self.set_path.is_none() {
            return Err(self.failed(id, "save the set before autosampling"));
        }
        // Spec: multisampler R68. The render borrows the engine's plug-ins,
        // so the song is stopped, as an export stops it.
        if self.audio.transport_state().playing {
            return Err(self.failed(id, "stop the song before autosampling"));
        }
        let settings = Settings {
            lowest: args.lowest.unwrap_or(36),
            highest: args.highest.unwrap_or(84),
            step: args.step.unwrap_or(3),
            velocity_layers: args.velocity_layers.unwrap_or(1),
            note_length: args.note_length.unwrap_or(2.0),
            release_tail: args.release_tail.unwrap_or(1.0),
            round_robins: args.round_robins.unwrap_or(1),
            detect_loops: args.detect_loops.unwrap_or(false),
            name: args.name.unwrap_or_else(|| source.name.clone()),
        }
        .checked();
        let track = self
            .editor
            .document()
            .device_track(device)
            .ok_or_else(|| self.failed(id, "the instrument isn't on a track"))?;
        let plan = autosample::plan(&settings);
        if plan.samples.is_empty() {
            return Err(self.failed(id, "the settings make no samples"));
        }
        // Spec: multisampler R69. A folder that holds files is never reused,
        // so a run can't overwrite, or on Cancel delete, an earlier run's.
        let base = self.processed_folder(Kind::Autosample);
        let stem = bl_project::preset::file_safe(&settings.name);
        let mut folder = base.join(&stem);
        let mut n = 2u32;
        while std::fs::read_dir(&folder)
            .map_or(folder.exists(), |mut entries| entries.next().is_some())
        {
            folder = base.join(format!("{stem} {n}"));
            n += 1;
        }
        self.autosample_result_value = None;
        self.autosample = Some(Job {
            device,
            track,
            settings,
            folder,
            plan,
            index: 0,
            written: Vec::new(),
            media: Vec::new(),
            layers: Vec::new(),
            without_loops: Vec::new(),
            files: Vec::new(),
            started: Instant::now(),
        });
        if args.wait.unwrap_or(true) {
            // The agent waits for the samples, so the result is complete.
            while self.autosample.is_some() {
                if let Err(error) = self.step_autosample(id) {
                    self.stop_autosample();
                    return Err(error);
                }
            }
            let result = self
                .autosample_result_value
                .take()
                .ok_or_else(|| self.failed(id, "the autosample didn't finish"))?;
            return Ok(to_value(&result));
        }
        Ok(to_value(&ids::Autosampled {
            track: None,
            device: None,
            files: Vec::new(),
            without_loops: Vec::new(),
            took: 0.0,
        }))
    }

    /// Makes the next sample, or lands the result when there are none left
    /// (R67 to R73). Returns whether a sample is still running.
    pub(super) fn poll_autosample(&mut self) -> bool {
        if self.autosample.is_none() {
            return false;
        }
        // A track landing inside a drag would be thrown away by its next frame.
        if self.landings_wait() {
            return true;
        }
        let id = ids::AUTOSAMPLE;
        match self.step_autosample(id) {
            Ok(running) => running,
            Err(error) => {
                self.set_message(error.to_string(), true);
                self.stop_autosample();
                false
            }
        }
    }

    /// Makes the next sample.
    fn step_autosample(&mut self, id: &str) -> Result<bool, ShellError> {
        let Some((device, sample, settings)) = self.next_sample() else {
            return Ok(false);
        };
        let frames = self.render_sample(id, device, sample.note, sample.velocity, &settings)?;
        let (path, media, length, channels) = self.write_sample(id, &settings, &sample, &frames)?;
        let mut layer = autosample::layer(
            &sample,
            &autosample::file_name(&settings, &sample),
            media,
            length,
            self.engine_rate(),
            channels,
        );
        let mut found = None;
        if settings.detect_loops {
            found = self.find_loop(&settings, &frames, channels as u16);
            match found {
                Some(loop_points) => {
                    layer.sustain = SustainMode::Loop;
                    layer.release = ReleaseMode::Release;
                    layer.loop_start = loop_points.start;
                    layer.loop_end = loop_points.end;
                    layer.release_loop = loop_points.end;
                    layer.crossfade_ms = LOOP_CROSSFADE_MS;
                }
                None => {
                    let name = autosample::file_name(&settings, &sample);
                    if let Some(job) = self.autosample.as_mut() {
                        job.without_loops.push(name);
                    }
                }
            }
        }
        if let Some(job) = self.autosample.as_mut() {
            job.written.push(path.clone());
            job.files.push(ids::AutosampleFile {
                path: path.to_string_lossy().into_owned(),
                note: sample.note,
                velocity: sample.velocity,
                round_robin: sample.round_robin + 1,
                layer: LayerId(0),
                loop_start: found.map(|loop_points| loop_points.start),
                loop_end: found.map(|loop_points| loop_points.end),
            });
            job.layers.push(layer);
            job.index += 1;
        }
        if self
            .autosample
            .as_ref()
            .is_some_and(|job| job.index >= job.plan.samples.len())
        {
            self.land_autosample(id)?;
            return Ok(false);
        }
        Ok(true)
    }

    /// The sample to make next: the instrument, the sample and the settings.
    fn next_sample(&self) -> Option<(DeviceId, Planned, Settings)> {
        let job = self.autosample.as_ref()?;
        let sample = job.plan.samples.get(job.index)?.clone();
        Some((job.device, sample, job.settings.clone()))
    }

    /// Stops the run, deleting the files it wrote (R73).
    pub(super) fn stop_autosample(&mut self) -> bool {
        let Some(job) = self.autosample.take() else {
            return false;
        };
        for path in &job.written {
            let _ = std::fs::remove_file(path);
        }
        // Only removes the folder when it is empty.
        let _ = std::fs::remove_dir(&job.folder);
        true
    }

    /// Renders the instrument alone with one note, as an export does (R67):
    /// the device on a track of its own, with the note, and nothing else.
    fn render_sample(
        &self,
        id: &str,
        device: DeviceId,
        note: u8,
        velocity: u8,
        settings: &Settings,
    ) -> Result<Vec<f32>, ShellError> {
        let rate = self.engine_rate();
        let frames = (settings.sample_seconds() * f64::from(rate))
            .round()
            .max(1.0) as u64;
        let document = self.render_document(id, device, note, velocity, settings)?;
        let mut samples: Vec<f32> = Vec::with_capacity(frames as usize * 2);
        let spec = crate::export::Render {
            start: Ticks::ZERO,
            length: crate::export::Length::Frames(frames),
            sample_rate: rate,
            engine_rate: rate,
            block: crate::export::BLOCK,
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::export::render(
            &document,
            self.media.clone(),
            self.plugin_hosts(),
            &spec,
            60.0,
            &cancel,
            &mut |_| {},
            &mut |block| {
                samples.extend_from_slice(block);
                Ok(())
            },
            None,
        )
        .map_err(|error| self.failed(id, error.to_string()))?;
        Ok(samples)
    }

    /// The document that renders the instrument alone with one note (R67):
    /// a MIDI track with the instrument and the note, and nothing else.
    fn render_document(
        &self,
        id: &str,
        device: DeviceId,
        note: u8,
        velocity: u8,
        settings: &Settings,
    ) -> Result<Document, ShellError> {
        autosample::render_document(self.editor.document(), device, note, velocity, settings)
            .map_err(|error| self.failed(id, error))
    }

    /// Writes the sample and joins it to the set's media (R69).
    fn write_sample(
        &mut self,
        id: &str,
        settings: &Settings,
        sample: &Planned,
        frames: &[f32],
    ) -> Result<(PathBuf, ContentHash, u64, u8), ShellError> {
        let name = autosample::file_name(settings, sample);
        let folder = match self.autosample.as_ref() {
            Some(job) => job.folder.clone(),
            None => return Err(self.failed(id, "the autosample was stopped")),
        };
        std::fs::create_dir_all(&folder).map_err(|error| {
            self.failed(id, format!("couldn't make {}: {error}", folder.display()))
        })?;
        let path = folder.join(format!("{name}.wav"));
        if path.exists() {
            return Err(self.failed(id, format!("{} already exists", path.display())));
        }
        let rate = self.engine_rate();
        let channels = 2u16;
        let mut writer = AudioWriter::create(
            &path,
            EncodeSpec {
                file_type: FileType::Wav,
                bit_depth: BitDepth::Float32,
                dither: Dither::None,
                sample_rate: rate,
                channels,
            },
        )
        .map_err(|error| self.failed(id, error.to_string()))?;
        writer
            .write(frames)
            .map_err(|error| self.failed(id, error.to_string()))?;
        writer
            .finish()
            .map_err(|error| self.failed(id, error.to_string()))?;
        let hash = self
            .media
            .import(&path)
            .map_err(|error| self.failed(id, error.to_string()))?;
        let entry = self
            .media
            .entry(hash)
            .ok_or_else(|| self.failed(id, "the media store lost the file"))?;
        let media = ContentHash(*hash.as_bytes());
        if let Some(job) = self.autosample.as_mut() {
            job.media.push((media, entry.path.clone(), entry.size));
        }
        let length = frames.len() as u64 / u64::from(channels);
        Ok((path, media, length, channels as u8))
    }

    /// The loop in a sample, when it has one (R71).
    fn find_loop(
        &self,
        settings: &Settings,
        frames: &[f32],
        channels: u16,
    ) -> Option<bl_analysis::loops::Loop> {
        let rate = self.engine_rate();
        let left: Vec<f32> = frames
            .chunks(channels as usize)
            .map(|frame| frame[0])
            .collect();
        let held = (settings.note_length * f64::from(rate)).round() as u64;
        bl_analysis::loops::find_loop(&left, rate, held)
    }

    /// The new track and its multisampler, as one undo step (R70).
    fn land_autosample(&mut self, id: &str) -> Result<(), ShellError> {
        // The job stays until the track lands, so a failure here still deletes
        // the files (R73).
        let Some(job) = self.autosample.as_ref() else {
            return Ok(());
        };
        let look = &self.prefs.look;
        let (device, track) = self
            .editor
            .transact(ids::STEP, |editor| {
                let parent = editor.document().parent(job.track);
                let siblings: Vec<TrackId> = match parent {
                    Some(group) => editor
                        .document()
                        .track(group)
                        .map(|group| group.children().collect())
                        .unwrap_or_default(),
                    None => editor.document().top_level().collect(),
                };
                let below = siblings
                    .iter()
                    .position(|id| *id == job.track)
                    .map_or(0, |index| index + 1);
                let track: TrackId = editor.new_id();
                editor.apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Midi,
                    name: job.settings.name.clone(),
                    parent,
                    index: below,
                })?;
                crate::colours::colour_new(editor, look, &[track], &[])?;
                let device: DeviceId = editor.new_id();
                let index = crate::devices::default_index(
                    editor.document(),
                    track,
                    DeviceRole::Instrument,
                    None,
                );
                editor.apply(Command::InsertDevice {
                    track,
                    index,
                    device: bl_devices::multisampler::multisampler_type().device(device),
                })?;
                // Spec: multisampler R70. Round Robin on in Forward mode
                // where there are round robins, and the volume envelope's
                // release as long as the release tail.
                if job.settings.round_robins > 1 {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param: bl_devices::multisampler::ROUND_ROBIN,
                        value: 1.0,
                    })?;
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param: bl_devices::multisampler::RR_MODE,
                        value: 0.0,
                    })?;
                }
                editor.apply(Command::SetDeviceParam {
                    device,
                    param: bl_devices::multisampler::ENV_RELEASE,
                    value: job.settings.release_tail.min(20.0),
                })?;
                editor.apply(Command::InsertLayers {
                    device,
                    index: None,
                    layers: job.layers.clone(),
                })?;
                Ok::<_, bl_model::EditError>((device, track))
            })
            .map_err(|error| self.failed(id, error))?;
        let Some(job) = self.autosample.take() else {
            return Ok(());
        };
        // The samples join the set's media now, with the track that plays
        // them (R69).
        for (hash, path, size) in &job.media {
            self.store.add_media(*hash, path, *size);
        }
        // The layers have their own IDs now, in list order.
        let mut files = job.files;
        for (file, layer) in files.iter_mut().zip(self.layers_of(device).list.iter()) {
            file.layer = layer.id;
        }
        self.autosample_result_value = Some(ids::Autosampled {
            track: Some(track),
            device: Some(device),
            files,
            without_loops: job.without_loops.clone(),
            took: job.started.elapsed().as_secs_f64(),
        });
        self.changed();
        Ok(())
    }
}

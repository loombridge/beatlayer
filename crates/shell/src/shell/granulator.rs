//! Granulator III's capture at work (WP-8.27; the Granulator III spec, R19,
//! R21 to R24): the command starts and stops a capture in the node, and
//! when it ends a worker writes the audio to a WAV file in the project's
//! `Samples/Recorded` folder while the shell makes the device play it, as
//! one undo step.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use bl_devices::granulator_iii::{
    CAPTURE_LENGTH, CAPTURE_MAX_SECONDS, CaptureBuffer, CaptureCommand, GRANULATOR_III,
};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{Command, Device, DeviceId, DeviceKind, Input};
use bl_shell_spec::granulator::{self as ids, GranulatorCaptureAction};

use super::*;

/// The undo step a finished capture is.
const UNDO_LABEL: &str = "Capture Audio";

/// A capture the shell is following: running, or over and being written.
pub(super) struct Capture {
    buffer: Arc<CaptureBuffer>,
    /// The file, in `Samples/Recorded`.
    path: PathBuf,
    rate: u32,
    /// The worker's report, once the capture is over and the file is being
    /// written.
    writing: Option<Receiver<Result<(), String>>>,
}

impl Shell {
    /// Whether `granulator.capture` can run now.
    pub(super) fn can_granulator(&self, args: &Value) -> bool {
        self.granulator_of(args)
            .is_some_and(|_| self.audio.engine_running())
    }

    /// The Granulator III the arguments name, or the selected device.
    fn granulator_of(&self, args: &Value) -> Option<Device> {
        let device = args
            .get("device")
            .and_then(|device| serde_json::from_value::<DeviceId>(device.clone()).ok())
            .or_else(|| self.devices.selected())?;
        self.editor
            .document()
            .device(device)
            .filter(|device| {
                matches!(&device.kind, DeviceKind::Native { device_type } if device_type == GRANULATOR_III)
            })
            .cloned()
    }

    /// Runs `granulator.capture`.
    pub(super) fn run_granulator(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ids::GranulatorCaptureArgs { device, action } = Self::args(id, args)?;
        let found = self
            .granulator_of(&json!({ "device": device }))
            .ok_or_else(|| self.failed(id, "select a Granulator III first"))?;
        self.poll_granulator_captures();
        let capturing = self
            .granulator_captures
            .get(&found.id)
            .is_some_and(|capture| !capture.buffer.finished());
        match action {
            GranulatorCaptureAction::Start => {
                if capturing {
                    return Err(self.failed(id, "a capture is already running"));
                }
                self.start_capture(id, &found)?;
            }
            GranulatorCaptureAction::Stop => {
                if capturing {
                    self.audio
                        .send_node_data(bl_engine::NodeKey::Device(found.id), CaptureCommand::Stop)
                        .map_err(|error| self.failed(id, error))?;
                }
            }
        }
        Ok(to_value(&ids::GranulatorCaptureResult {
            device: found.id,
            capturing: action == GranulatorCaptureAction::Start,
        }))
    }

    /// Spec: granulator iii R18, R19. Refuses a capture with nothing to
    /// capture, or nowhere to put the file; otherwise makes the buffer,
    /// off the audio thread and as long as Capture Length, and hands it to
    /// the node.
    fn start_capture(&mut self, id: &str, device: &Device) -> Result<(), ShellError> {
        let sidechain = device.sidechain.as_ref();
        if sidechain.is_none_or(|s| s.input == Input::None) {
            return Err(self.failed(
                id,
                "the Source is No Input; choose a track to capture first",
            ));
        }
        if sidechain.is_some_and(|s| !s.enabled) {
            return Err(self.failed(id, "the sidechain's External is off; switch it on first"));
        }
        let folder = self
            .set_path
            .as_ref()
            .map(|set| {
                ProjectFolder::of_set(set)
                    .root()
                    .join("Samples")
                    .join("Recorded")
            })
            .ok_or_else(|| {
                self.failed(
                    id,
                    "the set has never been saved; use `set.save_as` to give it a project \
                     folder, where its captures go",
                )
            })?;
        let rate = self
            .audio
            .sample_rate()
            .filter(|_| self.audio.engine_running())
            .ok_or_else(|| {
                self.failed(
                    id,
                    "no audio engine is running; switch it on and open an audio device first",
                )
            })?;
        let length = device
            .params
            .get(&CAPTURE_LENGTH)
            .copied()
            .unwrap_or(10.0)
            .clamp(1.0, CAPTURE_MAX_SECONDS);
        // The engine plays the set as it is now before the node starts.
        if !self.audio.flush(std::time::Duration::from_secs(10)) {
            return Err(self.failed(id, "the audio engine did not take the set"));
        }
        let label = self
            .editor
            .document()
            .device_track(device.id)
            .map_or_else(String::new, |track| {
                crate::routing::track_label(self.editor.document(), track)
            });
        let stem = format!(
            "{} Granulator III Capture [{}]",
            crate::recording::clean_label(&label),
            crate::recording::stamp()
        );
        // Two captures in one second don't share a file.
        let mut path = folder.join(format!("{stem}.wav"));
        for n in 2.. {
            if !path.exists() {
                break;
            }
            path = folder.join(format!("{stem} {n}.wav"));
        }
        let buffer = Arc::new(CaptureBuffer::new(
            (f64::from(rate) * length).round() as usize
        ));
        self.audio
            .send_node_data(
                bl_engine::NodeKey::Device(device.id),
                CaptureCommand::Start(Arc::clone(&buffer)),
            )
            .map_err(|error| self.failed(id, error))?;
        self.granulator_captures.insert(
            device.id,
            Capture {
                buffer,
                path,
                rate,
                writing: None,
            },
        );
        Ok(())
    }

    /// Spec: granulator iii R21, R22. Follows the captures: one that has
    /// ended is handed to a worker that writes its file, and once the file
    /// is written the device plays it, as one undo step.
    pub fn poll_granulator_captures(&mut self) {
        self.granulator_captures
            .retain(|device, _| self.editor.document().device(*device).is_some());
        let devices: Vec<DeviceId> = self.granulator_captures.keys().copied().collect();
        for device in devices {
            let Some(capture) = self.granulator_captures.get_mut(&device) else {
                continue;
            };
            // A node that is gone (the engine stopped) never ends its capture.
            if !self.audio.engine_running() {
                capture.buffer.end();
            }
            if capture.writing.is_none() && capture.buffer.finished() {
                if capture.buffer.filled() == 0 {
                    self.granulator_captures.remove(&device);
                    self.set_message("the capture ended before it held any audio", true);
                    continue;
                }
                capture.writing = Some(write_capture(
                    capture.buffer.read(),
                    capture.path.clone(),
                    capture.rate,
                ));
            }
            let Some(done) = capture.writing.as_ref().and_then(|w| w.try_recv().ok()) else {
                continue;
            };
            self.finish_capture(device, done);
        }
    }

    /// Waits for the files of the captures that have ended to be written
    /// and applies them, so a save or a test never leaves one half done.
    pub fn finish_granulator_captures(&mut self) {
        self.poll_granulator_captures();
        let devices: Vec<DeviceId> = self.granulator_captures.keys().copied().collect();
        for device in devices {
            let Some(capture) = self.granulator_captures.get_mut(&device) else {
                continue;
            };
            let Some(writing) = &capture.writing else {
                continue;
            };
            let done = writing
                .recv()
                .unwrap_or_else(|_| Err("the writer stopped".to_owned()));
            self.finish_capture(device, done);
        }
    }

    fn finish_capture(&mut self, device: DeviceId, done: Result<(), String>) {
        let Some(capture) = self.granulator_captures.remove(&device) else {
            return;
        };
        let applied = done.and_then(|()| self.use_capture(device, &capture.path));
        match applied {
            Ok(()) => {
                self.granulator_files.insert(device, capture.path);
            }
            Err(error) => self.set_message(format!("couldn't keep the capture: {error}"), true),
        }
    }

    /// The device's sample becomes the file, which the set references.
    fn use_capture(&mut self, device: DeviceId, path: &Path) -> Result<(), String> {
        let hash = self.media.import(path).map_err(|e| e.to_string())?;
        let entry = self
            .media
            .entry(hash)
            .ok_or("the media store lost the file")?;
        let media = bl_model::ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        self.editor
            .transact(UNDO_LABEL, |editor| {
                editor.apply(Command::SetDeviceSample {
                    device,
                    media: Some(media),
                    slot: 0,
                })
            })
            .map_err(|e| e.to_string())?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(())
    }

    /// Spec: granulator iii R23. What the query tool's `device` result
    /// reports under `capture` for a Granulator III: the source, Length,
    /// whether it is capturing, the frames captured and the file of the last
    /// capture.
    #[must_use]
    pub fn capture_of(&self, device: &Device) -> Option<Value> {
        let granulator = matches!(&device.kind, DeviceKind::Native { device_type } if device_type == GRANULATOR_III);
        if !granulator {
            return None;
        }
        let running = self.granulator_captures.get(&device.id);
        Some(json!({
            "source": device.sidechain.as_ref().map(|s| &s.input),
            "length": device.params.get(&CAPTURE_LENGTH).copied().unwrap_or(10.0),
            "capturing": running.is_some_and(|c| !c.buffer.finished()),
            "frames": running.map_or(0, |c| c.buffer.filled()),
            "file": self.granulator_files.get(&device.id),
        }))
    }
}

/// Writes a capture's two channels to `path` on a worker: 32-bit float WAV,
/// so the file holds the captured samples to the bit.
fn write_capture(
    [left, right]: [Vec<f32>; 2],
    path: PathBuf,
    rate: u32,
) -> Receiver<Result<(), String>> {
    let (done, report) = mpsc::channel();
    let work = move || -> Result<(), String> {
        let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| failed(&e))?;
        }
        let mut writer = AudioWriter::create(
            &path,
            EncodeSpec {
                file_type: FileType::Wav,
                bit_depth: BitDepth::Float32,
                dither: Dither::None,
                sample_rate: rate,
                channels: 2,
            },
        )
        .map_err(|e| failed(&e))?;
        let interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        writer.write(&interleaved).map_err(|e| failed(&e))?;
        writer.finish().map_err(|e| failed(&e))?;
        Ok(())
    };
    let sent = done.clone();
    if std::thread::Builder::new()
        .name("bl-shell-capture".into())
        .spawn(move || {
            let _ = done.send(work());
        })
        .is_err()
    {
        let _ = sent.send(Err("couldn't start the capture's writer".to_owned()));
    }
    report
}

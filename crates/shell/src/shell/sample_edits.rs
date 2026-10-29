//! Sample editing in the sampler devices (WP-5.15, IMP-28; the multisampler
//! spec, R54 to R63): the edit list kept with Simpler's sample and with each
//! multisampler layer, Crop and Reverse as processed copies, Simpler's
//! Normalize Volumes, the conversions between the two devices, and
//! auditioning from a point.
//!
//! Each edit goes through the document's own commands, so it is one undo
//! step and the source file is never written.

use std::sync::Arc;

use bl_devices::{multisampler as mosaic, sampler};
use bl_media::SampleData;
use bl_model::{
    Command, ContentHash, DeviceId, DeviceKind, Layer, LayerId, ParamId, SampleEdit, SampleRange,
    SampleRegion, SustainMode,
};
use bl_shell_spec::multisampler::{
    self as ids, AuditionArgs, Auditioned, ConvertArgs, ConvertTo, Converted, EditSampleArgs,
    Normalize, ProcessSampleArgs, SampleEdited, SampleProcessed, SampleRegionSet,
    SetSampleRegionArgs,
};

use super::*;
use crate::processed::{Kind, Request};

/// The sample a command acts on: Simpler's, or one layer's of a
/// multisampler.
#[derive(Clone, Copy, Debug)]
pub(super) struct Target {
    pub device: DeviceId,
    pub layer: Option<LayerId>,
}

/// A sample and the edits made to it.
struct Sample {
    media: ContentHash,
    edits: Vec<SampleEdit>,
}

impl Shell {
    /// Whether `device` is Simpler.
    pub(super) fn is_snapshot(device: &bl_model::Device) -> bool {
        matches!(
            &device.kind,
            DeviceKind::Native { device_type } if device_type == sampler::SAMPLER
        )
    }

    /// The sample `device` (or the selected device) and `layer` name: a
    /// multisampler's layer (its first selected if none is named), or
    /// Simpler's own sample.
    pub(super) fn sample_target(
        &self,
        id: &str,
        device: Option<DeviceId>,
        layer: Option<LayerId>,
    ) -> Result<Target, ShellError> {
        let device =
            device
                .or_else(|| self.devices.selected())
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
        let found =
            self.editor
                .document()
                .device(device)
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("no {device}"),
                })?;
        if DeviceView::is_multisampler(found) {
            let layer = match layer {
                Some(layer) => layer,
                None => *self
                    .layers_or_selection(id, device, None)?
                    .first()
                    .ok_or_else(|| self.failed(id, "select a layer first"))?,
            };
            if found.layers.get(layer).is_none() {
                return Err(self.failed(id, format!("no layer {}", layer.0)));
            }
            Ok(Target {
                device,
                layer: Some(layer),
            })
        } else if Self::is_snapshot(found) {
            if layer.is_some() {
                return Err(self.failed(id, "Simpler has one sample and no layers"));
            }
            Ok(Target {
                device,
                layer: None,
            })
        } else {
            Err(self.failed(id, format!("{device} isn't a sampler")))
        }
    }

    /// The sample `target` plays and its edits.
    fn sample_of(&self, id: &str, target: Target) -> Result<Sample, ShellError> {
        let device = self
            .editor
            .document()
            .device(target.device)
            .ok_or_else(|| self.failed(id, format!("no {}", target.device)))?;
        match target.layer {
            Some(layer) => {
                let layer = device
                    .layers
                    .get(layer)
                    .ok_or_else(|| self.failed(id, format!("no layer {}", layer.0)))?;
                Ok(Sample {
                    media: layer.media,
                    edits: layer.edits.clone(),
                })
            }
            None => Ok(Sample {
                media: device
                    .media_at(0)
                    .ok_or_else(|| self.failed(id, "load a sample first"))?,
                edits: device.sample_edits.clone(),
            }),
        }
    }

    /// The sample with its edits applied, at its own rate (R57): shared with
    /// the engine and the display while any of them holds it. `None` if the
    /// media store can't supply the file or an edit doesn't fit it.
    #[must_use]
    pub fn edited_audio(
        &self,
        media: ContentHash,
        edits: &[SampleEdit],
    ) -> Option<Arc<SampleData>> {
        self.media
            .load_edited(
                bl_media::ContentHash::from_bytes(media.0),
                &bl_engine::media_edits(edits),
            )
            .ok()
    }

    /// The sample being auditioned from a click in a display: its device
    /// and layer, while the button is down.
    #[must_use]
    pub fn auditioning(&self) -> Option<(DeviceId, Option<LayerId>)> {
        self.auditioning
    }

    /// Runs one of the sample commands of WP-5.15.
    pub(super) fn run_sample_edits(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::EDIT_SAMPLE => {
                let args: EditSampleArgs = Self::args(id, args)?;
                self.edit_sample(id, &args)
            }
            ids::CROP_SAMPLE | ids::REVERSE_SAMPLE => {
                let ProcessSampleArgs { device, layer } = Self::args(id, args)?;
                let target = self.sample_target(id, device, layer)?;
                self.process_sample(id, target, id == ids::CROP_SAMPLE)
            }
            ids::CONVERT => {
                let args: ConvertArgs = Self::args(id, args)?;
                self.convert(id, &args)
            }
            ids::SET_SAMPLE_REGION => {
                let args: SetSampleRegionArgs = Self::args(id, args)?;
                self.set_sample_region(id, &args)
            }
            ids::AUDITION_SAMPLE => {
                let args: AuditionArgs = Self::args(id, args)?;
                let target = self.sample_target(id, args.device, args.layer)?;
                let audition = bl_engine::Audition {
                    layer: target.layer,
                    from: args.from.clamp(0.0, 1.0),
                    down: args.down,
                };
                self.auditioning = args.down.then_some((target.device, target.layer));
                let sent = self.audio.audition(target.device, audition).is_ok();
                Ok(to_value(&Auditioned { sent }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// `device.edit_sample` (R55, R58).
    fn edit_sample(&mut self, id: &str, args: &EditSampleArgs) -> Result<Value, ShellError> {
        let target = self.sample_target(id, args.device, args.layer)?;
        let Target { device, layer } = target;
        let sample = self.sample_of(id, target)?;
        let source = bl_media::ContentHash::from_bytes(sample.media.0);
        if args.revert {
            if args.kind.is_some() || args.range.is_some() {
                return Err(self.failed(id, "revert takes no kind or range"));
            }
            if !sample.edits.is_empty() {
                let length = self
                    .media
                    .load(source)
                    .map_err(|error| self.failed(id, error))?
                    .frames() as u64;
                self.editor
                    .apply(Command::ClearSampleEdits {
                        device,
                        layer,
                        length,
                    })
                    .map_err(|error| self.failed(id, error))?;
                self.changed();
            }
        } else {
            let kind = args
                .kind
                .ok_or_else(|| self.failed(id, "name the edit's kind, or revert"))?;
            let edit = SampleEdit {
                kind,
                range: args.range,
            };
            // The selection must fit the sample as the earlier edits left
            // it; a layer's model checks that against its length, and
            // Simpler's is checked against the sample here.
            if layer.is_none() {
                let length = self
                    .edited_audio(sample.media, &sample.edits)
                    .ok_or_else(|| self.failed(id, "the sample's file is missing"))?
                    .frames() as u64;
                edit.length_after(length)
                    .map_err(|error| self.failed(id, error))?;
            }
            self.editor
                .apply(Command::AddSampleEdit {
                    device,
                    layer,
                    edit,
                })
                .map_err(|error| self.failed(id, error))?;
            self.changed();
        }
        let sample = self.sample_of(id, target)?;
        let length = self
            .edited_audio(sample.media, &sample.edits)
            .map_or(0, |audio| audio.frames() as u64);
        Ok(to_value(&SampleEdited {
            device,
            layer,
            edits: sample.edits,
            length,
        }))
    }

    /// Simpler's Start and End flags as frames of its edited sample, which is
    /// `frames` long (the sampler spec, R22).
    fn snapshot_region(&self, device: DeviceId, frames: u64) -> (u64, u64) {
        self.editor
            .document()
            .device(device)
            .and_then(|d| d.region)
            .map_or((0, frames), |region| region.frames_in(frames))
    }

    /// The part Classic mode plays, in frames of the edited sample: S Start's
    /// share of the region for S Length's (R3), as the node works it out.
    fn snapshot_played(&self, device: DeviceId, frames: u64) -> (u64, u64) {
        let (first, last) = self.snapshot_region(device, frames);
        let description = sampler::sampler_type().description;
        let (start, length) = self
            .editor
            .document()
            .device(device)
            .map_or((0.0, 1.0), |d| {
                let value = |id| description.value(&d.params, id).unwrap_or_default() / 100.0;
                (value(sampler::START), value(sampler::LENGTH))
            });
        let span = last.saturating_sub(first) as f64;
        let from = first + (start * span) as u64;
        (
            from,
            (first + ((start + length).min(1.0) * span) as u64).min(last),
        )
    }

    /// `device.set_sample_region` (R22): the flags in frames, an End past the
    /// sample coming back to it. Flags that cover the whole sample are no
    /// region.
    fn set_sample_region(
        &mut self,
        id: &str,
        args: &SetSampleRegionArgs,
    ) -> Result<Value, ShellError> {
        let target = self.sample_target(id, args.device, None)?;
        let device = target.device;
        let sample = self.sample_of(id, target)?;
        let length = self
            .edited_audio(sample.media, &sample.edits)
            .ok_or_else(|| self.failed(id, "the sample's file is missing"))?
            .frames() as u64;
        let (was_start, was_end) = self.snapshot_region(device, length);
        let end = args.end.unwrap_or(was_end).min(length);
        let start = args.start.unwrap_or(was_start);
        if start >= end {
            return Err(self.failed(
                id,
                format!(
                    "Start ({start}) must be before End ({end}) in a sample of {length} frames"
                ),
            ));
        }
        let region = (start, end) != (0, length);
        self.editor
            .apply(Command::SetDeviceRegion {
                device,
                region: region.then_some(SampleRegion::Frames { start, end }),
            })
            .map_err(|error| self.failed(id, error))?;
        self.changed();
        Ok(to_value(&SampleRegionSet {
            device,
            start,
            end,
            length,
        }))
    }

    /// `device.crop_sample` and `device.reverse_sample` (R59): the edited
    /// sample, or its region, written as a processed copy that the device
    /// or layer then plays. The copy's name is in the project's
    /// `Samples/Processed/` folder.
    fn process_sample(
        &mut self,
        id: &str,
        target: Target,
        crop: bool,
    ) -> Result<Value, ShellError> {
        let Target { device, layer } = target;
        let sample = self.sample_of(id, target)?;
        let source = bl_media::ContentHash::from_bytes(sample.media.0);
        if self.set_path.is_none() {
            let reason = format!(
                "{} needs the set saved first: a set that has never been saved has no project \
                 folder for the copy. Use File ▸ Save Live Set As…",
                if crop { "Crop" } else { "Reverse" }
            );
            self.set_message(reason.clone(), true);
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            });
        }
        let audio = self
            .edited_audio(sample.media, &sample.edits)
            .ok_or_else(|| self.failed(id, "the sample's file is missing"))?;
        let frames = audio.frames() as u64;
        let (from, to) = if !crop {
            (0, frames)
        } else if let Some(layer) = layer {
            let layer = self
                .layers_of(device)
                .get(layer)
                .cloned()
                .ok_or_else(|| self.failed(id, format!("no layer {}", layer.0)))?;
            (layer.start, layer.end.min(frames))
        } else {
            self.snapshot_region(device, frames)
        };
        if to <= from || (crop && from == 0 && to == frames) {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: if crop {
                    "the region is the whole sample; there is nothing to crop".to_owned()
                } else {
                    "the sample has no audio".to_owned()
                },
            });
        }
        let stem = self
            .media
            .entry(source)
            .and_then(|entry| {
                entry
                    .path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Sample".to_owned());
        let label = if crop { "Crop" } else { "Reverse" };
        let channels = audio.channel_count();
        let request = Request {
            kind: if crop { Kind::Crop } else { Kind::Reverse },
            name: format!("{stem} {label}.wav"),
            spec: bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: audio.sample_rate(),
                channels: channels as u16,
            },
            frames: to - from,
            produce: Box::new(move |sink| {
                let (from, to) = (from as usize, to as usize);
                let mut buffer = Vec::with_capacity(4096 * channels);
                for chunk_start in (from..to).step_by(4096) {
                    buffer.clear();
                    for k in chunk_start..(chunk_start + 4096).min(to) {
                        // Reverse reads from the end.
                        let frame = if crop { k } else { to - 1 - (k - from) };
                        for channel in 0..channels {
                            buffer.push(audio.channel(channel).map_or(0.0, |c| c[frame]));
                        }
                    }
                    sink.write(&buffer)?;
                }
                Ok(())
            }),
        };
        let landed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = std::rc::Rc::clone(&landed);
        let length = to - from;
        self.start_processed(request, move |shell, written| {
            let media = ContentHash(*written.hash.as_bytes());
            let crop_range = crop.then_some(SampleRange { from, to });
            shell
                .editor
                .transact(label, |editor| match layer {
                    Some(layer) => editor
                        .apply(Command::SetProcessedLayerSample {
                            device,
                            layer,
                            media,
                            length,
                            crop: crop_range,
                        })
                        .map(drop),
                    None => {
                        // Loading the copy clears Simpler's edits and region,
                        // so a crop's region is the whole of the copy.
                        editor
                            .apply(Command::SetDeviceSample {
                                device,
                                media: Some(media),
                                slot: 0,
                            })
                            .map(drop)
                    }
                })
                .map_err(|error| error.to_string())?;
            shell.changed();
            *slot.borrow_mut() = Some((media, written));
            Ok(())
        })
        .map_err(|error| self.failed(id, error))?;
        // shortcut: the command waits for the file, so the person's click
        // blocks the window for the copy's write; upgrade to returning at
        // once with the Status Bar's progress when samples run long.
        self.wait_processed();
        let (media, written) = landed
            .borrow_mut()
            .take()
            .ok_or_else(|| self.failed(id, "the copy couldn't be written"))?;
        Ok(to_value(&SampleProcessed {
            device,
            layer,
            media,
            path: written.path.to_string_lossy().into_owned(),
            length,
        }))
    }

    /// Simpler's Normalize Volumes (R60): its Gain, so the region's largest
    /// sample magnitude reaches 0 dBFS.
    pub(super) fn normalize_snapshot(
        &mut self,
        id: &str,
        device: DeviceId,
        what: Normalize,
    ) -> Result<Value, ShellError> {
        if what == Normalize::Pan {
            return Err(self.failed(id, "Simpler has no Normalize Pan"));
        }
        let target = Target {
            device,
            layer: None,
        };
        let sample = self.sample_of(id, target)?;
        let audio = self
            .edited_audio(sample.media, &sample.edits)
            .ok_or_else(|| self.failed(id, "the sample's file is missing"))?;
        let (from, to) = self.snapshot_region(device, audio.frames() as u64);
        let peak = audio
            .channels()
            .flat_map(|c| c[from as usize..to as usize].iter())
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        if peak > 0.0 {
            // The Gain parameter is a linear gain, up to +24 dB.
            let gain = (1.0 / f64::from(peak)).min(bl_model::LAYER_GAIN_MAX);
            self.editor
                .transact("Normalize Volume", |editor| {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param: sampler::GAIN,
                        value: gain,
                    })
                })
                .map_err(|error| self.failed(id, error))?;
            self.changed();
        }
        Ok(to_value(&bl_shell_spec::multisampler::LayersChanged {
            device,
            layers: Vec::new(),
        }))
    }

    /// `device.convert` (R62): Simpler and the multisampler into each
    /// other, in the old one's place.
    fn convert(&mut self, id: &str, args: &ConvertArgs) -> Result<Value, ShellError> {
        let device = args
            .device
            .or_else(|| self.devices.selected())
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "select a device first".to_owned(),
            })?;
        let old = self
            .editor
            .document()
            .device(device)
            .cloned()
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("no {device}"),
            })?;
        let place = self
            .editor
            .document()
            .device_place(device)
            .ok_or_else(|| self.failed(id, format!("{device} isn't in a chain")))?;
        let new_id: DeviceId = self.editor.new_id();
        let mut dropped = Vec::new();
        let (new, layer) = match args.to {
            ConvertTo::Multisampler => {
                if !Self::is_snapshot(&old) {
                    return Err(self.failed(id, format!("{device} isn't Simpler")));
                }
                self.snapshot_to_multisampler(id, &old, new_id, &mut dropped)?
            }
            ConvertTo::Snapshot => {
                if !DeviceView::is_multisampler(&old) {
                    return Err(self.failed(id, format!("{device} isn't a multisampler")));
                }
                (
                    self.multisampler_to_snapshot(&old, new_id, &mut dropped),
                    None,
                )
            }
        };
        // A chain holds one instrument, so the old one goes first.
        self.editor
            .transact("Convert", |editor| {
                editor.apply(Command::RemoveDevice { device })?;
                match place.chain {
                    None => editor.apply(Command::InsertDevice {
                        track: place.track,
                        index: place.index,
                        device: new,
                    })?,
                    Some(chain) => editor.apply(Command::InsertChainDevice {
                        chain,
                        index: place.index,
                        device: new,
                    })?,
                };
                if let Some(layer) = layer {
                    editor.apply(Command::InsertLayers {
                        device: new_id,
                        index: None,
                        layers: vec![layer],
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|error| self.failed(id, error))?;
        self.devices.select_only(new_id);
        self.changed();
        Ok(to_value(&Converted {
            device: new_id,
            to: args.to,
            dropped,
        }))
    }

    /// Simpler as a multisampler's device and its one layer: the whole
    /// zones, the part Classic plays as Sample Start and Sample End, the loop
    /// as a sustain loop (R62).
    fn snapshot_to_multisampler(
        &self,
        id: &str,
        old: &bl_model::Device,
        new_id: DeviceId,
        dropped: &mut Vec<String>,
    ) -> Result<(bl_model::Device, Option<Layer>), ShellError> {
        let description = sampler::sampler_type().description;
        let value = |param| description.value(&old.params, param).unwrap_or_default();
        let mut new = mosaic::multisampler_type().device(new_id);
        new.enabled = old.enabled;
        if old.name != sampler::sampler_type().name {
            new.name.clone_from(&old.name);
        }
        let carry = [
            (sampler::VOLUME, mosaic::VOLUME),
            (sampler::PAN, mosaic::PAN),
            (sampler::VELOCITY, mosaic::VEL_VOL),
            (sampler::VOICES, mosaic::VOICES),
            (sampler::RETRIG, mosaic::RETRIGGER),
            (sampler::ATTACK, mosaic::ENV_ATTACK),
            (sampler::DECAY, mosaic::ENV_DECAY),
            (sampler::SUSTAIN, mosaic::ENV_SUSTAIN),
            (sampler::RELEASE, mosaic::ENV_RELEASE),
        ];
        for (from, to) in carry {
            new.params.insert(to, value(from));
        }
        if value(sampler::MODE) as usize != 0 {
            dropped.push(format!(
                "{} mode: the multisampler plays its layers as Classic does",
                sampler::MODES[value(sampler::MODE) as usize]
            ));
        }
        for (on, what) in [
            (sampler::FILTER, "the filter"),
            (sampler::LFO, "the LFO"),
            (sampler::GLIDE, "Glide"),
        ] {
            if value(on) >= 0.5 {
                dropped.push(format!("{what} (the multisampler's own come with WP-5.14)"));
            }
        }
        if value(sampler::TRANSPOSE) != 0.0 || value(sampler::DETUNE) != 0.0 {
            dropped.push("Transpose and Detune".to_owned());
        }
        let Some(media) = old.media_at(0) else {
            return Ok((new, None));
        };
        let audio = self
            .edited_audio(media, &old.sample_edits)
            .ok_or_else(|| self.failed(id, "the sample's file is missing"))?;
        let frames = audio.frames() as u64;
        let (start, end) = self.snapshot_played(old.id, frames);
        let name = self
            .media
            .entry(bl_media::ContentHash::from_bytes(media.0))
            .and_then(|entry| {
                entry
                    .path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Sample".to_owned());
        let mut layer = Layer::new(
            LayerId(0),
            name,
            media,
            frames,
            audio.sample_rate(),
            audio.channel_count() as u8,
        );
        layer.edits.clone_from(&old.sample_edits);
        layer.start = start;
        layer.end = end.max(start + 1).min(frames);
        layer.loop_start = start;
        layer.loop_end = layer.end;
        layer.release_loop = layer.end;
        layer.volume = value(sampler::GAIN).min(bl_model::LAYER_GAIN_MAX);
        if value(sampler::LOOP) >= 0.5 {
            let share = value(sampler::LOOP_LENGTH) / 100.0;
            let span = layer.end - layer.start;
            layer.loop_start = layer.end - ((share * span as f64) as u64).clamp(1, span);
            layer.sustain = SustainMode::Loop;
            let loop_ms =
                (layer.loop_end - layer.loop_start) as f64 / f64::from(layer.sample_rate) * 1000.0;
            layer.crossfade_ms = (value(sampler::FADE) / 100.0 * loop_ms).min(loop_ms / 2.0);
        }
        Ok((new, Some(layer)))
    }

    /// A multisampler as Simpler in Classic mode playing its first layer
    /// (R62).
    fn multisampler_to_snapshot(
        &self,
        old: &bl_model::Device,
        new_id: DeviceId,
        dropped: &mut Vec<String>,
    ) -> bl_model::Device {
        let mut new = sampler::sampler_type().device(new_id);
        new.enabled = old.enabled;
        if old.name != mosaic::multisampler_type().name {
            new.name.clone_from(&old.name);
        }
        let description = mosaic::multisampler_type().description;
        let value = |param| description.value(&old.params, param).unwrap_or_default();
        let mut set = |param: ParamId, value: f64| {
            new.params.insert(param, value);
        };
        for (from, to) in [
            (mosaic::VOLUME, sampler::VOLUME),
            (mosaic::VEL_VOL, sampler::VELOCITY),
            (mosaic::RETRIGGER, sampler::RETRIG),
            (mosaic::ENV_ATTACK, sampler::ATTACK),
            (mosaic::ENV_DECAY, sampler::DECAY),
            (mosaic::ENV_SUSTAIN, sampler::SUSTAIN),
            (mosaic::ENV_RELEASE, sampler::RELEASE),
        ] {
            set(to, value(from));
        }
        set(
            sampler::VOICES,
            value(mosaic::VOICES).min(sampler::MAX_VOICES as f64),
        );
        set(sampler::MODE, 0.0);
        let Some(layer) = old.layers.list.first() else {
            return new;
        };
        set(
            sampler::PAN,
            (value(mosaic::PAN) + layer.pan).clamp(-50.0, 50.0),
        );
        set(sampler::GAIN, layer.volume);
        // Sample Start and Sample End become Simpler's flags (R62).
        let whole = (layer.start, layer.end) == (0, layer.length);
        new.region = (!whole && layer.start < layer.end).then_some(SampleRegion::Frames {
            start: layer.start,
            end: layer.end,
        });
        // Simpler plays C3 at the sample's own pitch; the layer's root key
        // is where the multisampler does.
        set(
            sampler::TRANSPOSE,
            (60.0 - f64::from(layer.root_key)).clamp(-48.0, 48.0),
        );
        set(sampler::DETUNE, layer.detune.clamp(-50.0, 50.0));
        if layer.sustain != SustainMode::Off {
            let region = (layer.end - layer.start).max(1) as f64;
            set(sampler::LOOP, 1.0);
            set(
                sampler::LOOP_LENGTH,
                ((layer.end - layer.loop_start) as f64 / region * 100.0).clamp(0.0, 100.0),
            );
            let loop_ms = (layer.loop_end - layer.loop_start) as f64
                / f64::from(layer.sample_rate.max(1))
                * 1000.0;
            if loop_ms > 0.0 {
                set(
                    sampler::FADE,
                    (layer.crossfade_ms / loop_ms * 100.0).clamp(0.0, 100.0),
                );
            }
        }
        new.media = vec![Some(layer.media)];
        new.sample_edits.clone_from(&layer.edits);
        if old.layers.list.len() > 1 {
            dropped.push(format!(
                "{} more layers, and every layer's zones",
                old.layers.list.len() - 1
            ));
        }
        if layer.release != bl_model::ReleaseMode::Off {
            dropped.push("the layer's release mode".to_owned());
        }
        // Simpler's loop always ends at the region's end, plays forwards
        // and has no detune of its own.
        if layer.sustain != SustainMode::Off && layer.loop_end != layer.end {
            dropped.push("the layer's Loop End".to_owned());
        }
        if layer.sustain == SustainMode::BackAndForth {
            dropped.push("the layer's back-and-forth loop".to_owned());
        }
        if layer.sustain != SustainMode::Off && layer.loop_detune != 0.0 {
            dropped.push("the layer's loop detune".to_owned());
        }
        if old.layers.list.len() == 1
            && (layer.key != bl_model::Zone::whole(bl_model::ZoneKind::Key)
                || layer.velocity != bl_model::Zone::whole(bl_model::ZoneKind::Velocity)
                || layer.select != bl_model::Zone::whole(bl_model::ZoneKind::Select))
        {
            dropped.push("the layer's zones".to_owned());
        }
        new
    }
}

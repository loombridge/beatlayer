//! Push 2's Convert (`docs/specs/push-2-notes.md`, R76): what the button
//! makes of the selected Simpler, drum rack pad or audio clip, each one undo
//! step.

use bl_control::Convert;
use bl_devices::sampler as sampler_ids;
use bl_model::{
    Chain, ChainId, ClipData, ClipId, Command, Device, DeviceId, DeviceRole, RackKind, Receive,
    TrackId, TrackKind,
};
use bl_shell_spec::push::{PushConvertTo, PushConverted};
use serde_json::{Value, json};

use super::slice::{
    FIRST_NOTE, PLAY_NOTE, failed, invalid, is_sampler, sampler_device, unavailable,
};
use super::*;

/// The undo step's name.
const STEP: &str = "Convert";

/// What Convert acts on, by what is selected.
enum Target {
    /// A Simpler in Classic or One-Shot mode.
    Simpler(TrackId, DeviceId),
    /// A Simpler in Slice mode.
    Slices(DeviceId),
    /// A drum rack's selected pad.
    Pad(TrackId, DeviceId, u8),
    /// An audio clip.
    Clip(ClipId),
}

impl Shell {
    /// What Convert acts on: the selected device on the track shown, else
    /// the selected audio clip (R76).
    fn convert_target(&self) -> Option<Target> {
        let doc = self.editor.document();
        let shown = crate::devices::shown_track(&self.arrangement, doc)?;
        let selected = self
            .devices
            .selected()
            .filter(|device| doc.device_track(*device) == Some(shown));
        if let Some(device) = selected.and_then(|id| doc.device(id)) {
            if is_sampler(device) {
                let description = sampler_ids::sampler_type().description;
                let mode = description
                    .value(&device.params, sampler_ids::MODE)
                    .unwrap_or_default();
                return Some(if mode >= 2.0 {
                    Target::Slices(device.id)
                } else {
                    Target::Simpler(shown, device.id)
                });
            }
            if device.rack_kind() == Some(RackKind::Drum)
                && let Some(note) = self
                    .devices
                    .drum_view(doc, shown)
                    .filter(|view| view.rack == device.id)
                    .map(|view| view.selected.unwrap_or(view.first))
            {
                return Some(Target::Pad(shown, device.id, note));
            }
        }
        self.slice_clip(&json!({})).ok().map(Target::Clip)
    }

    /// What a surface's Convert would act on (R76).
    pub(super) fn surface_convert(&self) -> Convert {
        match self.convert_target() {
            None => Convert::Nothing,
            Some(Target::Simpler(..)) => Convert::Simpler,
            Some(Target::Slices(_)) => Convert::Slices,
            Some(Target::Pad(..)) => Convert::Pad,
            Some(Target::Clip(_)) => Convert::Clip,
        }
    }

    /// `push.convert` (R76).
    pub(super) fn push_convert(
        &mut self,
        id: &str,
        to: Option<PushConvertTo>,
    ) -> Result<Value, ShellError> {
        let target = self
            .convert_target()
            .ok_or_else(|| unavailable(id, "select a Simpler, a drum rack pad or an audio clip"))?;
        let converted = match target {
            Target::Simpler(track, device) => self.convert_simpler(id, track, device)?,
            Target::Slices(device) => {
                let slices = self.simpler_slices(id, Some(device))?;
                let made = self.slice_to_drum_rack(id, &slices)?;
                PushConverted {
                    from: "slices".to_owned(),
                    track: None,
                    device: serde_json::from_value(made["device"].clone())
                        .map_err(|error| failed(id, error))?,
                }
            }
            Target::Pad(track, rack, note) => self.convert_pad(id, track, rack, note)?,
            Target::Clip(clip) => {
                self.convert_clip(id, clip, to.unwrap_or(PushConvertTo::Simpler))?
            }
        };
        Ok(to_value(&converted))
    }

    /// A copy of `device` with IDs of its own.
    fn copy_of(&mut self, device: &Device) -> Device {
        let mut copy = device.clone();
        copy.reassign_ids(&mut || self.editor.new_id());
        copy
    }

    /// A drum rack with `device` on its first pad, which plays at the
    /// device's own pitch (R76).
    fn drum_rack_of(&mut self, device: Device) -> Device {
        let rack_id: DeviceId = self.editor.new_id();
        let chain_id: ChainId = self.editor.new_id();
        let mut rack = Device::rack(rack_id, RackKind::Drum);
        let mut chain =
            Chain::for_pad(chain_id, device.name.clone(), FIRST_NOTE).with_devices(vec![device]);
        chain.play = PLAY_NOTE;
        rack.chains.push(chain);
        rack
    }

    /// A new MIDI track holding `devices`, after `after`, as one undo step
    /// named [`STEP`], and selected.
    fn insert_converted_track(
        &mut self,
        id: &str,
        after: TrackId,
        name: String,
        devices: Vec<Device>,
    ) -> Result<TrackId, ShellError> {
        let track: TrackId = self.editor.new_id();
        let (parent, index) = self.track_after(after);
        self.editor
            .transact(STEP, |editor| {
                editor.apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Midi,
                    name,
                    parent,
                    index,
                })?;
                for (at, device) in devices.into_iter().enumerate() {
                    editor.apply(Command::InsertDevice {
                        track,
                        index: at,
                        device,
                    })?;
                }
                crate::colours::colour_new(editor, &self.prefs.look, &[track], &[])
            })
            .map_err(|error| failed(id, error))?;
        self.dirty = true;
        self.changed();
        let _ = self.arrangement.run(
            view_ids::SELECT_TRACKS,
            json!({ "tracks": [track] }),
            self.editor.document(),
        );
        Ok(track)
    }

    /// A Simpler in Classic or One-Shot mode: a new track with a drum rack
    /// holding a copy of it on the pad for note 36, and copies of the track's
    /// other devices in their places (R76).
    fn convert_simpler(
        &mut self,
        id: &str,
        track: TrackId,
        simpler: DeviceId,
    ) -> Result<PushConverted, ShellError> {
        let (name, devices): (String, Vec<Device>) = {
            let held = self
                .editor
                .document()
                .track(track)
                .ok_or_else(|| invalid(id, "no track has that ID"))?;
            (held.name().to_owned(), held.devices().cloned().collect())
        };
        let mut made = Vec::new();
        let mut rack_id = simpler;
        for device in &devices {
            let copy = self.copy_of(device);
            if device.id == simpler {
                let rack = self.drum_rack_of(copy);
                rack_id = rack.id;
                made.push(rack);
            } else {
                made.push(copy);
            }
        }
        let new = self.insert_converted_track(id, track, name, made)?;
        Ok(PushConverted {
            from: "simpler".to_owned(),
            track: Some(new),
            device: rack_id,
        })
    }

    /// A drum rack's selected pad: a new track holding copies of the devices
    /// of the pad's first chain (R76).
    fn convert_pad(
        &mut self,
        id: &str,
        track: TrackId,
        rack: DeviceId,
        note: u8,
    ) -> Result<PushConverted, ShellError> {
        let chain = self
            .editor
            .document()
            .device(rack)
            .and_then(|rack| {
                rack.chains
                    .iter()
                    .find(|chain| !chain.is_return && chain.receive == Receive::Note(note))
                    .cloned()
            })
            .ok_or_else(|| unavailable(id, "the selected pad holds no devices"))?;
        let made: Vec<Device> = chain
            .devices
            .iter()
            .map(|device| self.copy_of(device))
            .collect();
        let first = made
            .iter()
            .find(|device| device.role == DeviceRole::Instrument)
            .or_else(|| made.first())
            .map(|device| device.id)
            .ok_or_else(|| unavailable(id, "the selected pad holds no devices"))?;
        let new = self.insert_converted_track(id, track, chain.name, made)?;
        Ok(PushConverted {
            from: "pad".to_owned(),
            track: Some(new),
            device: first,
        })
    }

    /// An audio clip: a new track with a Simpler holding its sample, or a
    /// drum rack holding that on its first pad, with the clip's warp
    /// settings and copies of the track's devices (R76).
    fn convert_clip(
        &mut self,
        id: &str,
        clip: ClipId,
        to: PushConvertTo,
    ) -> Result<PushConverted, ShellError> {
        let (audio, track, name, devices) = {
            let doc = self.editor.document();
            let found = doc
                .clip(clip)
                .ok_or_else(|| invalid(id, "no clip has that ID"))?;
            let ClipData::Audio(audio) = found.data.clone() else {
                return Err(unavailable(id, "the clip isn't an audio clip"));
            };
            let track = doc
                .clip_track(clip)
                .ok_or_else(|| invalid(id, "no clip has that ID"))?;
            let held = doc
                .track(track)
                .ok_or_else(|| invalid(id, "no track has that ID"))?;
            (
                audio,
                track,
                if found.name.is_empty() {
                    held.name().to_owned()
                } else {
                    found.name.clone()
                },
                held.devices().cloned().collect::<Vec<Device>>(),
            )
        };
        let hash = bl_media::ContentHash::from_bytes(audio.media.0);
        let info = self
            .media
            .info(hash)
            .map_err(|_| failed(id, "the clip's file isn't in the media store"))?;
        let file_seconds = self
            .file_seconds(&audio)
            .ok_or_else(|| failed(id, "the clip's file isn't in the media store"))?;
        // The flags: the part the clip plays, in frames of the file.
        let rate = f64::from(info.sample_rate);
        let (from, to_seconds) = audio.crop_region(file_seconds);
        let frames = info.frames.unwrap_or((file_seconds * rate) as u64);
        let region = Some((
            (from * rate).round() as u64,
            (to_seconds * rate).round() as u64,
        ));
        let simpler_id: DeviceId = self.editor.new_id();
        let mut simpler = sampler_device(simpler_id, audio.media, 0.0, region, &[], frames);
        simpler.warp = bl_model::DeviceWarp {
            warp: audio.warp,
            mode: audio.warp_mode,
            settings: audio.warp_settings,
            bpm: audio.tempo,
        };
        simpler
            .params
            .insert(sampler_ids::TRANSPOSE, f64::from(audio.transpose));
        simpler.params.insert(sampler_ids::DETUNE, audio.detune);
        let instrument = match to {
            PushConvertTo::Simpler => simpler,
            PushConvertTo::DrumRack => self.drum_rack_of(simpler),
        };
        let device = instrument.id;
        let mut made = vec![instrument];
        for other in &devices {
            made.push(self.copy_of(other));
        }
        let new = self.insert_converted_track(id, track, name, made)?;
        Ok(PushConverted {
            from: "clip".to_owned(),
            track: Some(new),
            device,
        })
    }
}

//! Rack and chain presets, and loading presets by hot-swap (WP-5.8; the
//! racks spec, R42 to R47; the device chains spec, R19).
//!
//! A preset file holds a device's values, a whole rack or one chain
//! ([`bl_project::rack_preset::Saved`]). Rack and chain presets carry the
//! samples their devices play; loading one puts those in the set's media
//! store, and into the User Library when no file of theirs is there.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bl_media::MediaStore;
use bl_model::{
    ChainId, Command, ContentHash, Device, DeviceId, DeviceKind, DeviceRole, Editor, OrdMap,
    ParamId, TrackId, TrackKind,
};
use bl_project::preset::{self, Preset};
use bl_project::rack_preset::{ChainPreset, RackPreset, SampleFile, Samples, Saved};
use bl_shell_spec::devices::{
    AddDeviceArgs, AddPresetArgs, ChainAdded, ChainPresetResult, DeviceAdded, LoadChainPresetArgs,
    NewDevice, ReplaceArgs, SaveChainPresetArgs,
};

use super::{DeviceError, DeviceView, Outcome, map_inner_copies, new_device, no_library};
use crate::browser::arrange::MediaFile;

/// Where a preset's samples go when the set's media store lacks them,
/// under the User Library.
const SAMPLES_FOLDER: &str = "Samples/Preset Samples";

/// The values a device preset sets once the device is in place.
type State = (
    OrdMap<ParamId, f64>,
    String,
    Option<Vec<bl_model::modulator::ShapePoint>>,
);

/// A device made ready to go in a chain, with new IDs: what a preset or a
/// default holds.
pub(super) struct Ready {
    /// The device, and everything in it.
    pub(super) device: Device,
    /// A device preset's values, set once the device is in.
    pub(super) state: Option<State>,
    /// The device the copy was made from, whose modulators map the copies
    /// of their targets inside it (the modulation spec, R19).
    pub(super) original: Option<Device>,
    /// The default preset's name, if a default made it.
    pub(super) default_name: Option<String>,
}

fn failed(error: impl ToString) -> DeviceError {
    DeviceError::Failed(error.to_string())
}

fn media_hash(hash: &ContentHash) -> bl_media::ContentHash {
    bl_media::ContentHash::from_bytes(hash.0)
}

/// The samples `hashes` name, read from the files the media store has them
/// in.
fn gather(media: &MediaStore, hashes: &BTreeSet<ContentHash>) -> Result<Samples, DeviceError> {
    let mut samples = Samples::new();
    for hash in hashes {
        let entry = media.entry(media_hash(hash)).ok_or_else(|| {
            DeviceError::Failed(format!("the media store doesn't know the sample {hash}"))
        })?;
        let data = fs::read(&entry.path).map_err(|e| {
            DeviceError::Failed(format!(
                "{}: {e}; the preset needs the sample it plays",
                entry.path.display()
            ))
        })?;
        if bl_media::ContentHash::of_bytes(&data) != media_hash(hash) {
            return Err(DeviceError::Failed(format!(
                "{} no longer holds the sample the device plays",
                entry.path.display()
            )));
        }
        let name = entry.path.file_name().map_or_else(
            || "sample".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        samples.insert(*hash, SampleFile { name, data });
    }
    Ok(samples)
}

/// Makes sure the set can find the samples a preset carries: any the media
/// store has a file for stay where they are, and the rest are written to the
/// User Library, imported, and recorded in `outcome` for the set's media
/// table.
fn bring_in(
    media: &MediaStore,
    library: Option<&Path>,
    samples: &Samples,
    outcome: &mut Outcome,
) -> Result<(), DeviceError> {
    for (hash, file) in samples {
        let wanted = media_hash(hash);
        let entry = match media.entry(wanted).filter(|entry| entry.path.is_file()) {
            Some(entry) => entry,
            None => {
                if bl_media::ContentHash::of_bytes(&file.data) != wanted {
                    return Err(DeviceError::Failed(format!(
                        "the preset's sample {} is damaged",
                        file.name
                    )));
                }
                let folder = library.ok_or_else(no_library)?.join(SAMPLES_FOLDER);
                let path = folder.join(format!(
                    "{}-{}",
                    &hash.to_string()[..16],
                    preset::file_safe(&file.name)
                ));
                fs::create_dir_all(&folder).map_err(failed)?;
                if !path.is_file() {
                    fs::write(&path, &file.data).map_err(failed)?;
                }
                if media.import(&path).map_err(failed)? != wanted {
                    return Err(DeviceError::Failed(format!(
                        "{} doesn't hold the preset's sample",
                        path.display()
                    )));
                }
                media.entry(wanted).ok_or_else(|| {
                    DeviceError::Failed("the media store lost the file".to_owned())
                })?
            }
        };
        outcome.media.push(MediaFile {
            hash: *hash,
            path: entry.path,
            size: entry.size,
        });
    }
    Ok(())
}

/// Reads a preset file of any kind, and brings in the samples a rack or
/// chain preset carries.
pub(super) fn open_preset(
    path: &Path,
    media: &MediaStore,
    library: Option<&Path>,
    outcome: &mut Outcome,
) -> Result<Saved, DeviceError> {
    let saved = Saved::load(path).map_err(failed)?;
    match &saved {
        Saved::Rack(preset) => bring_in(media, library, &preset.samples, outcome)?,
        Saved::Chain(preset) => bring_in(media, library, &preset.samples, outcome)?,
        Saved::Device(_) => {}
    }
    Ok(saved)
}

/// Whether the file at `path` holds a chain preset.
pub fn is_chain_preset(path: &Path) -> bool {
    matches!(Saved::load(path), Ok(Saved::Chain(_)))
}

/// Writes `device`'s state to `path`: a rack whole, with the samples its
/// devices play, as a rack preset; any other device as a device preset.
pub(super) fn save_device(
    device: &Device,
    media: &MediaStore,
    path: &Path,
) -> Result<(), DeviceError> {
    if device.rack_kind().is_none() {
        return Preset::of(device).save(path).map_err(failed);
    }
    let samples = gather(media, &device.media_used())?;
    RackPreset::new(device.clone(), samples)
        .and_then(|preset| preset.save(path))
        .map_err(failed)
}

impl DeviceView {
    /// A new device of `kind` as the host offers its type, or as the preset
    /// names it for a type it doesn't know (a plug-in that isn't scanned).
    fn blank(&self, id: DeviceId, kind: &DeviceKind, name: &str, role: DeviceRole) -> Device {
        if let DeviceKind::Rack { kind } = kind {
            return Device::rack(id, *kind);
        }
        match self.types.iter().find(|t| t.kind == *kind) {
            Some(device_type) => device_type.device(id),
            None => Device::new(id, name, kind.clone()).with_role(role),
        }
    }

    /// The device a preset makes, ready to go in a chain: a device preset's
    /// device with its values, or a rack with new IDs for everything in it.
    /// `name` is the preset's.
    pub(super) fn ready(
        &self,
        editor: &mut Editor,
        saved: Saved,
        name: &str,
    ) -> Result<Ready, DeviceError> {
        match saved {
            Saved::Device(p) => {
                let id: DeviceId = editor.new_id();
                let device = self.blank(id, &p.kind, &p.device_name, p.role);
                let state = (self.sanitize(&p.kind, p.params), name.to_owned(), p.shape);
                Ok(Ready {
                    device,
                    state: Some(state),
                    original: None,
                    default_name: None,
                })
            }
            Saved::Rack(preset) => {
                let rack = preset.rack;
                let mut device = rack.clone();
                device.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
                device.clear_mappings();
                Ok(Ready {
                    device,
                    state: None,
                    original: Some(rack),
                    default_name: None,
                })
            }
            Saved::Chain(_) => Err(DeviceError::Unavailable(
                "that is a chain preset; `chain.load_preset` loads it".to_owned(),
            )),
        }
    }

    /// The default preset of a new rack, if the User Library has one for
    /// its kind (the device chains spec, R18; the racks spec, R43).
    pub(super) fn default_rack(
        &self,
        editor: &mut Editor,
        args: &AddDeviceArgs,
        library: Option<&Path>,
        media: &MediaStore,
        outcome: &mut Outcome,
    ) -> Option<Ready> {
        let DeviceKind::Rack { kind } = args.device.kind else {
            return None;
        };
        let path = preset::default_path(library?, kind.role(), kind.name());
        if !path.is_file() {
            return None;
        }
        let saved = open_preset(&path, media, library, outcome).ok()?;
        let Saved::Rack(rack) = &saved else {
            return None;
        };
        if rack.rack.rack_kind() != Some(kind) {
            return None;
        }
        let mut ready = self
            .ready(editor, saved, &preset::preset_name(&path))
            .ok()?;
        ready.default_name = Some(preset::preset_name(&path));
        Some(ready)
    }

    /// `device.add_preset`: what a preset file holds, ready to go where
    /// `device.add` would put it, with the arguments that say where.
    pub(super) fn ready_to_add(
        &self,
        args: &AddPresetArgs,
        editor: &mut Editor,
        library: Option<&Path>,
        media: &MediaStore,
        outcome: &mut Outcome,
    ) -> Result<(AddDeviceArgs, Ready), DeviceError> {
        let path = super::preset_path(&args.path)?;
        let saved = open_preset(&path, media, library, outcome)?;
        let ready = self.ready(editor, saved, &preset::preset_name(&path))?;
        let new = NewDevice {
            kind: ready.device.kind.clone(),
            name: ready.device.title().to_owned(),
            role: ready.device.role,
            sidechain: ready.device.sidechain.is_some(),
            midi_sidechain: ready.device.midi_sidechain.is_some(),
        };
        let add = AddDeviceArgs {
            device: new,
            track: args.track,
            chain: args.chain,
            index: args.index,
        };
        Ok((add, ready))
    }

    /// `device.replace`: a device or rack replaced by another of its role.
    pub(super) fn replace(
        &mut self,
        args: ReplaceArgs,
        editor: &mut Editor,
        library: Option<&Path>,
        media: &MediaStore,
        outcome: &mut Outcome,
    ) -> Result<DeviceAdded, DeviceError> {
        let ReplaceArgs {
            device,
            chain,
            preset,
            new_device: new,
        } = args;
        let (ready, label) = match (preset, new) {
            (Some(path), None) => {
                let path = super::preset_path(&path)?;
                let saved = open_preset(&path, media, library, outcome)?;
                if let (Saved::Device(loaded), None) = (&saved, chain) {
                    // Spec: device-chains R16, racks R45. A preset of the
                    // device's own kind loads into it.
                    let target = self.replace_target(editor, device)?;
                    let current = editor.document().device(target).expect("the target exists");
                    if loaded.fits(current) {
                        super::load_preset(self, editor, target, &path)?;
                        return Ok(self.added(editor, target));
                    }
                }
                (
                    self.ready(editor, saved, &preset::preset_name(&path))?,
                    "Load Preset",
                )
            }
            (None, Some(new)) => {
                let id: DeviceId = editor.new_id();
                let device = new_device(
                    id,
                    &AddDeviceArgs {
                        device: new,
                        track: None,
                        chain: None,
                        index: None,
                    },
                );
                let ready = Ready {
                    device,
                    state: None,
                    original: None,
                    default_name: None,
                };
                (ready, "Insert Device")
            }
            _ => {
                return Err(DeviceError::InvalidArgs(
                    "give one of `preset` and `new_device`".to_owned(),
                ));
            }
        };
        if let Some(chain) = chain {
            return self.replace_chain_device(editor, chain, ready, label);
        }
        let target = self.replace_target(editor, device)?;
        self.replace_device(editor, target, ready, label)
    }

    /// The device `device.replace` replaces: the one named, or the one
    /// being hot-swapped, or the selected one.
    fn replace_target(
        &self,
        editor: &Editor,
        device: Option<DeviceId>,
    ) -> Result<DeviceId, DeviceError> {
        self.target(editor.document(), device.or(self.hot_swap))
    }

    fn added(&self, editor: &Editor, device: DeviceId) -> DeviceAdded {
        let place = editor
            .document()
            .device_place(device)
            .expect("the device has a place");
        DeviceAdded {
            device,
            track: place.track,
            new_track: false,
            default_preset: None,
        }
    }

    /// Replaces `target` with `ready` in its place as one undo step named
    /// `label`; hot-swap stays on for the replacement.
    fn replace_device(
        &mut self,
        editor: &mut Editor,
        target: DeviceId,
        ready: Ready,
        label: &'static str,
    ) -> Result<DeviceAdded, DeviceError> {
        let doc = editor.document();
        let old = doc.device(target).expect("the target exists");
        let new = &ready.device;
        if old.role != new.role {
            return Err(DeviceError::Unavailable(format!(
                "{} is {} and can't replace {}, {}",
                new.title(),
                new.role.label(),
                old.title(),
                old.role.label()
            )));
        }
        let place = doc.device_place(target).expect("the target has a place");
        let armed = self.hot_swap == Some(target);
        let Ready {
            device,
            state,
            original,
            ..
        } = ready;
        let id = device.id;
        let copies: Vec<DeviceId> = device.walk().map(|d| d.id).collect();
        let pairs: Vec<(&Device, DeviceId)> =
            original.iter().flat_map(Device::walk).zip(copies).collect();
        editor
            .transact(label, |editor| {
                // A chain holds one instrument, so the old one goes first.
                editor.apply(Command::RemoveDevice { device: target })?;
                match place.chain {
                    None => editor.apply(Command::InsertDevice {
                        track: place.track,
                        index: place.index,
                        device,
                    }),
                    Some(chain) => editor.apply(Command::InsertChainDevice {
                        chain,
                        index: place.index,
                        device,
                    }),
                }?;
                if let Some((params, name, shape)) = state {
                    editor.apply(Command::SetDeviceState {
                        device: id,
                        params,
                        name,
                        shape,
                    })?;
                }
                map_inner_copies(editor, &pairs)
            })
            .map_err(failed)?;
        self.selected = Some(id);
        if armed {
            self.hot_swap = Some(id);
        }
        Ok(DeviceAdded {
            device: id,
            track: place.track,
            new_track: false,
            default_preset: None,
        })
    }

    /// Makes `ready` the only device of `chain`, which keeps its name and
    /// mixer (the racks spec, R45).
    fn replace_chain_device(
        &mut self,
        editor: &mut Editor,
        chain: ChainId,
        ready: Ready,
        label: &'static str,
    ) -> Result<DeviceAdded, DeviceError> {
        let doc = editor.document();
        let track = doc
            .chain_track(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
        let old: Vec<DeviceId> = doc
            .chain(chain)
            .map(|c| c.devices.iter().map(|d| d.id).collect())
            .unwrap_or_default();
        let Ready {
            device,
            state,
            original,
            ..
        } = ready;
        let id = device.id;
        let copies: Vec<DeviceId> = device.walk().map(|d| d.id).collect();
        let pairs: Vec<(&Device, DeviceId)> =
            original.iter().flat_map(Device::walk).zip(copies).collect();
        editor
            .transact(label, |editor| {
                for device in old {
                    editor.apply(Command::RemoveDevice { device })?;
                }
                editor.apply(Command::InsertChainDevice {
                    chain,
                    index: 0,
                    device,
                })?;
                if let Some((params, name, shape)) = state {
                    editor.apply(Command::SetDeviceState {
                        device: id,
                        params,
                        name,
                        shape,
                    })?;
                }
                map_inner_copies(editor, &pairs)
            })
            .map_err(failed)?;
        self.selected = Some(id);
        Ok(DeviceAdded {
            device: id,
            track,
            new_track: false,
            default_preset: None,
        })
    }

    /// `device.load_preset` for a rack: the rack replaced by the preset's,
    /// of the same kind.
    pub(super) fn load_rack_preset(
        &mut self,
        editor: &mut Editor,
        target: DeviceId,
        saved: Saved,
        name: &str,
    ) -> Result<DeviceId, DeviceError> {
        let Saved::Rack(preset) = &saved else {
            return Err(DeviceError::Unavailable(
                "that is a chain preset; `chain.load_preset` loads it".to_owned(),
            ));
        };
        let current = editor.document().device(target).expect("the target exists");
        if current.rack_kind() != preset.rack.rack_kind() {
            return Err(DeviceError::Failed(format!(
                "a preset for {} can't load into {}",
                preset.rack.own_name(),
                current.own_name()
            )));
        }
        let ready = self.ready(editor, saved, name)?;
        self.replace_device(editor, target, ready, "Load Preset")
            .map(|added| added.device)
    }

    /// `chain.save_preset`: a chain written to a preset file.
    pub(super) fn save_chain_preset(
        &self,
        args: SaveChainPresetArgs,
        editor: &Editor,
        library: Option<&Path>,
        media: &MediaStore,
    ) -> Result<(ChainPresetResult, PathBuf), DeviceError> {
        let SaveChainPresetArgs {
            chain,
            path,
            folder,
        } = args;
        let doc = editor.document();
        let chain = match chain {
            Some(chain) => chain,
            None => self
                .chain_targets(doc, None)?
                .first()
                .copied()
                .ok_or_else(|| DeviceError::Unavailable("select a chain first".to_owned()))?,
        };
        let (rack, _) = doc
            .chain_place(chain)
            .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?;
        let rack = doc.device(rack).expect("the chain's rack exists");
        let content = doc.chain(chain).expect("the chain exists");
        let name = if content.name.is_empty() {
            "Chain"
        } else {
            &content.name
        };
        let path = match (path, folder) {
            (Some(_), Some(_)) => {
                return Err(DeviceError::InvalidArgs(
                    "give one of `path` and `folder`".to_owned(),
                ));
            }
            (Some(path), None) => super::preset_path(&path)?,
            (None, Some(folder)) => preset::unused_path(Path::new(&folder), name),
            (None, None) => {
                let library = library.ok_or_else(no_library)?;
                preset::unused_path(&preset::presets_folder(library, rack), name)
            }
        };
        let samples = gather(media, &content.media_used())?;
        ChainPreset {
            rack_kind: rack.rack_kind().expect("a rack"),
            chain: content.clone(),
            samples,
        }
        .save(&path)
        .map_err(failed)?;
        let result = ChainPresetResult {
            chain,
            name: preset::preset_name(&path),
            path: path.to_string_lossy().into_owned(),
        };
        Ok((result, path))
    }

    /// `chain.load_preset`: a chain preset added to a rack, or in place of
    /// a chain.
    pub(super) fn load_chain_preset(
        &mut self,
        args: LoadChainPresetArgs,
        editor: &mut Editor,
        library: Option<&Path>,
        media: &MediaStore,
        outcome: &mut Outcome,
    ) -> Result<ChainAdded, DeviceError> {
        let LoadChainPresetArgs {
            path,
            rack,
            index,
            chain,
        } = args;
        let path = super::preset_path(&path)?;
        let Saved::Chain(loaded) = open_preset(&path, media, library, outcome)? else {
            return Err(DeviceError::Unavailable(
                "that is not a chain preset".to_owned(),
            ));
        };
        let doc = editor.document();
        // The chain to replace: the one named, or the armed one when no
        // rack is named.
        let replaced = chain.or_else(|| self.hot_swap_chain.filter(|_| rack.is_none()));
        let (rack, at) = match replaced {
            Some(chain) => doc
                .chain_place(chain)
                .ok_or_else(|| DeviceError::InvalidArgs(format!("no {chain}")))?,
            None => {
                let rack = match rack {
                    Some(rack) => rack,
                    None => self
                        .hot_swap
                        .filter(|d| doc.device(*d).is_some_and(|d| d.rack_kind().is_some()))
                        .or_else(|| self.focus_rack(doc).map(|d| d.id))
                        .ok_or_else(|| {
                            DeviceError::Unavailable("select a rack first".to_owned())
                        })?,
                };
                let holder = doc
                    .device(rack)
                    .filter(|d| d.rack_kind().is_some())
                    .ok_or_else(|| DeviceError::InvalidArgs(format!("{rack} isn't a rack")))?;
                (
                    rack,
                    index
                        .unwrap_or(holder.chains.len())
                        .min(holder.chains.len()),
                )
            }
        };
        let Some(old) = replaced else {
            return self.insert_chain_copies(editor, std::slice::from_ref(&loaded.chain), rack, at);
        };
        let mut copy = loaded.chain.clone();
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        copy.devices.iter_mut().for_each(Device::clear_mappings);
        let new_id = copy.id;
        let pairs: Vec<(&Device, DeviceId)> = loaded
            .chain
            .devices_deep()
            .zip(copy.devices_deep().map(|d| d.id))
            .collect();
        editor
            .transact("Load Preset", |editor| {
                editor.apply(Command::AddChain {
                    rack,
                    index: at,
                    chain: copy,
                })?;
                editor.apply(Command::RemoveChain { chain: old })?;
                map_inner_copies(editor, &pairs)
            })
            .map_err(failed)?;
        self.select_new_chains(&[new_id]);
        if self.hot_swap_chain == Some(old) {
            self.hot_swap_chain = Some(new_id);
        }
        Ok(ChainAdded {
            chain: new_id,
            rack,
            chains: vec![new_id],
        })
    }

    /// The device `device.hot_swap` arms with none named or selected: the
    /// shown track's first audio effect, or its instrument on a MIDI track
    /// (the racks spec, R44).
    pub(super) fn first_hot_swap_target(
        doc: &bl_model::Document,
        track: Option<TrackId>,
    ) -> Option<DeviceId> {
        let track = doc.track(track?)?;
        let role = if track.kind() == TrackKind::Midi {
            DeviceRole::Instrument
        } else {
            DeviceRole::AudioEffect
        };
        track.devices().find(|d| d.role == role).map(|d| d.id)
    }
}

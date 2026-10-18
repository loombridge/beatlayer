//! The multisampler's commands that need the shell (WP-5.13 and WP-5.14; the
//! multisampler spec, R3, R7, R10, R11, R13, R20, R47): adding layers from
//! files, importing an SFZ file, changing layer settings with Snap,
//! replacing a layer's sample, normalising, and the views and selection.
//!
//! Each edit goes through the document's own layer commands, inside one
//! transaction where it takes several, so it's one undo step.

use std::collections::{BTreeSet, HashMap};

use bl_devices::multisampler as mosaic;
use bl_interop::sfz::{self, FilterKind, RoundRobin};
use bl_media::{SampleData, read_sampler_info};
use bl_model::{
    Command, ContentHash, Device, DeviceId, DeviceRole, Layer, LayerId, LayerSettings, ParamId,
    TrackId, TrackKind, ZoneKind, snap_to_zero_crossing,
};
use bl_shell_spec::multisampler::{
    self as ids, AddLayersArgs, HotSwapLayerArgs, ImportSfzArgs, LayerSelectMode, LayersAdded,
    LayersChanged, LoopKind, MultisamplerState, Normalize, NormalizeArgs, SelectLayersArgs,
    SetLayerArgs, SetLayerSampleArgs, SetViewArgs, SfzCount, SfzImported, SfzLeftOut,
    SnapMarkersArgs,
};

use super::*;

impl Shell {
    /// Runs one of the multisampler's shell commands.
    pub(super) fn run_multisampler(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::ADD_LAYERS => {
                let args: AddLayersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                self.add_layers(id, device, &args, args.index)
            }
            ids::IMPORT_SFZ => {
                let args: ImportSfzArgs = Self::args(id, args)?;
                self.import_sfz(id, args)
            }
            ids::SET_LAYER => {
                let args: SetLayerArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                let snap = self.layers_of(device).snap;
                let changed = self.edit_layers(
                    id,
                    "Change Sample Settings",
                    device,
                    &layers,
                    |_, settings| settings.clone(),
                    &args.settings,
                    snap,
                )?;
                Ok(to_value(&changed))
            }
            ids::SNAP_LAYER_MARKERS => {
                let args: SnapMarkersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                let which = args.which;
                let changed = self.edit_layers(
                    id,
                    "Snap Markers",
                    device,
                    &layers,
                    move |layer, _| match which {
                        LoopKind::Sustain => LayerSettings::markers(
                            None,
                            None,
                            Some(layer.loop_start),
                            Some(layer.loop_end),
                            None,
                        ),
                        LoopKind::Release => {
                            LayerSettings::markers(None, None, None, None, Some(layer.release_loop))
                        }
                    },
                    &LayerSettings::default(),
                    true,
                )?;
                Ok(to_value(&changed))
            }
            ids::SET_LAYER_SAMPLE => {
                let args: SetLayerSampleArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layer = match args.layer {
                    Some(layer) => layer,
                    None => *self
                        .layers_or_selection(id, device, None)?
                        .first()
                        .ok_or_else(|| self.failed(id, "select a layer first"))?,
                };
                let (media, length, rate, channels, _) = self.import_layer_file(id, &args.path)?;
                self.editor
                    .apply(Command::SetLayerMedia {
                        device,
                        layer,
                        media,
                        length,
                        sample_rate: rate,
                        channels,
                    })
                    .map_err(|error| self.failed(id, error))?;
                self.changed();
                Ok(to_value(&LayersChanged {
                    device,
                    layers: vec![layer],
                }))
            }
            ids::EDIT_SAMPLE
            | ids::CROP_SAMPLE
            | ids::REVERSE_SAMPLE
            | ids::CONVERT
            | ids::AUDITION_SAMPLE => self.run_sample_edits(id, args),
            ids::NORMALIZE_LAYERS => {
                let args: NormalizeArgs = Self::args(id, args)?;
                // Snapshot's Normalize Volumes sets its Gain (R60).
                let snapshot = args.device.or_else(|| self.devices.selected()).filter(|d| {
                    self.editor
                        .document()
                        .device(*d)
                        .is_some_and(Self::is_snapshot)
                });
                if let Some(device) = snapshot {
                    return self.normalize_snapshot(id, device, args.what);
                }
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                self.normalize(id, device, &layers, args.what)
            }
            ids::SET_VIEW => {
                let args: SetViewArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let mut view = self.devices.multisampler_view(device);
                view.tab = args.tab.unwrap_or(view.tab);
                view.zone_editor = args.zone_editor.unwrap_or(view.zone_editor);
                view.editor = args.editor.unwrap_or(view.editor);
                view.size = args.size.unwrap_or(view.size);
                view.auto_select = args.auto_select.unwrap_or(view.auto_select);
                self.devices.set_multisampler_view(device, view);
                self.touch();
                Ok(to_value(&self.multisampler_state(device)))
            }
            ids::SELECT_LAYERS => {
                let args: SelectLayersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                self.select(id, device, &args)?;
                self.touch();
                Ok(to_value(&self.multisampler_state(device)))
            }
            ids::HOT_SWAP_LAYER => {
                let args: HotSwapLayerArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                if args.on == Some(false) {
                    self.devices.set_layer_hot_swap(None);
                } else {
                    let layer = match args.layer {
                        Some(layer) => layer,
                        None => *self
                            .layers_or_selection(id, device, None)?
                            .first()
                            .ok_or_else(|| self.failed(id, "select a layer first"))?,
                    };
                    if self.layers_of(device).get(layer).is_none() {
                        return Err(self.failed(id, format!("no layer {}", layer.0)));
                    }
                    self.devices.set_layer_hot_swap(Some((device, layer)));
                }
                self.touch();
                Ok(to_value(&self.multisampler_state(device)))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The layer a loaded sample replaces, if one is being hot-swapped and
    /// the browser has one file selected (R10): the shell's `Load Selected`
    /// then runs `device.set_layer_sample` and ends the hot-swap.
    pub(super) fn hot_swapped_layer_file(&self) -> Option<(DeviceId, LayerId, PathBuf)> {
        let (device, layer) = self.devices.layer_hot_swap()?;
        match self.browser_view.selected() {
            [one] if one.is_file() => Some((device, layer, one.clone())),
            _ => None,
        }
    }

    pub(super) fn failed(&self, id: &str, message: impl ToString) -> ShellError {
        ShellError::Failed {
            command: id.to_owned(),
            message: message.to_string(),
        }
    }

    /// Records that the set changed.
    pub(super) fn changed(&mut self) {
        self.dirty = true;
        self.document_changed();
        self.touch();
    }

    /// The multisampler a command acts on: `device`, or the selected one.
    pub(super) fn multisampler_target(
        &self,
        id: &str,
        device: Option<DeviceId>,
    ) -> Result<DeviceId, ShellError> {
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
        if !DeviceView::is_multisampler(found) {
            return Err(self.failed(id, format!("{device} isn't a multisampler")));
        }
        Ok(device)
    }

    pub(super) fn layers_of(&self, device: DeviceId) -> bl_model::Layers {
        self.editor
            .document()
            .device(device)
            .map(|d| d.layers.clone())
            .unwrap_or_default()
    }

    /// `layers`, or the selected ones when none are given; they must all be
    /// there, and some must be.
    pub(super) fn layers_or_selection(
        &self,
        id: &str,
        device: DeviceId,
        layers: Option<Vec<LayerId>>,
    ) -> Result<Vec<LayerId>, ShellError> {
        let layers =
            layers.unwrap_or_else(|| self.devices.layer_selection(self.editor.document(), device));
        if layers.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "select a layer first".to_owned(),
            });
        }
        Ok(layers)
    }

    /// A multisampler's views and selection.
    fn multisampler_state(&self, device: DeviceId) -> MultisamplerState {
        MultisamplerState {
            view: self.devices.multisampler_view(device),
            selection: self.devices.layer_selection(self.editor.document(), device),
            layers: self.layers_of(device).list.len(),
            hot_swap: self
                .devices
                .layer_hot_swap()
                .filter(|(d, _)| *d == device)
                .map(|(_, layer)| layer),
        }
    }

    /// Imports a file into the set's media and reads what a layer needs of
    /// it: the media, its length, rate and channels, and the audio.
    fn import_layer_file(
        &mut self,
        id: &str,
        path: &str,
    ) -> Result<(ContentHash, u64, u32, u8, std::sync::Arc<SampleData>), ShellError> {
        let path = PathBuf::from(path);
        let hash = self
            .media
            .import(&path)
            .map_err(|error| self.failed(id, format!("{}: {error}", path.display())))?;
        let entry = self
            .media
            .entry(hash)
            .ok_or_else(|| self.failed(id, "the media store lost the file"))?;
        let data = self
            .media
            .load(hash)
            .map_err(|error| self.failed(id, format!("{}: {error}", path.display())))?;
        let media = ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        let channels = u8::try_from(data.channel_count()).unwrap_or(u8::MAX);
        Ok((
            media,
            data.frames() as u64,
            data.sample_rate(),
            channels,
            data,
        ))
    }

    /// Adds files as layers (R3).
    fn add_layers(
        &mut self,
        id: &str,
        device: DeviceId,
        args: &AddLayersArgs,
        index: Option<usize>,
    ) -> Result<Value, ShellError> {
        let mut layers = Vec::new();
        let mut skipped = Vec::new();
        let held = self.layers_of(device);
        let like = match args.like {
            Some(like) => Some(
                held.get(like)
                    .ok_or_else(|| self.failed(id, format!("no layer {}", like.0)))?
                    .clone(),
            ),
            None => None,
        };
        for path in &args.paths {
            let file = Path::new(path);
            match self.import_layer_file(id, path) {
                Ok((media, length, rate, channels, _)) if length > 0 => {
                    let name = file
                        .file_stem()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let mut layer = Layer::new(LayerId(0), name, media, length, rate, channels);
                    if let Some(like) = &like {
                        (layer.key, layer.velocity, layer.select) =
                            (like.key, like.velocity, like.select);
                    }
                    let info = read_sampler_info(file);
                    layer.root_key = info.root_key.unwrap_or(bl_model::DEFAULT_ROOT_KEY);
                    if let Some((start, end)) = info.loop_points
                        && end <= length
                    {
                        layer.loop_start = start;
                        layer.loop_end = end;
                        layer.release_loop = end;
                    }
                    layers.push(layer);
                }
                Ok(_) => skipped.push(format!("{path}: it has no audio")),
                Err(error) => skipped.push(match error {
                    ShellError::Failed { message, .. } => message,
                    other => other.to_string(),
                }),
            }
        }
        if layers.is_empty() {
            return Err(self.failed(
                id,
                if skipped.is_empty() {
                    "no files were given".to_owned()
                } else {
                    skipped.join("; ")
                },
            ));
        }
        let at = match args.after {
            Some(after) => Some(
                held.position(after)
                    .ok_or_else(|| self.failed(id, format!("no layer {}", after.0)))?
                    + 1,
            ),
            None => index,
        }
        .unwrap_or_else(|| {
            let selected = self.devices.layer_selection(self.editor.document(), device);
            held.list
                .iter()
                .rposition(|l| selected.contains(&l.id))
                .map_or(held.list.len(), |i| i + 1)
        });
        let count = layers.len();
        self.editor
            .apply(Command::InsertLayers {
                device,
                index: Some(at),
                layers,
            })
            .map_err(|error| self.failed(id, error))?;
        let added: Vec<LayerId> = self.layers_of(device).list[at..at + count]
            .iter()
            .map(|l| l.id)
            .collect();
        self.devices.select_layers(device, added.clone());
        self.changed();
        Ok(to_value(&LayersAdded {
            device,
            layers: added,
            skipped,
        }))
    }

    /// The layer one SFZ region makes (R49, R50), or why it can't be one:
    /// the sample is imported into the set's media, and what the file itself
    /// holds (a root key and a loop) fills in what the region leaves open.
    fn sfz_layer(
        &mut self,
        id: &str,
        region: &sfz::Region,
        opened: &mut HashMap<PathBuf, Result<OpenedSample, String>>,
    ) -> Result<Layer, String> {
        let file = &region.sample;
        // Regions share samples: a file is imported and measured once.
        let (media, length, rate, channels, info) = match opened.get(file) {
            Some(known) => known.clone()?,
            None => {
                let found = self
                    .import_layer_file(id, &file.to_string_lossy())
                    .map_err(|error| match error {
                        ShellError::Failed { message, .. } => message,
                        other => other.to_string(),
                    })
                    .and_then(|(media, length, rate, channels, _)| {
                        if length == 0 {
                            Err("it has no audio".to_owned())
                        } else {
                            Ok((media, length, rate, channels, read_sampler_info(file)))
                        }
                    });
                opened.insert(file.clone(), found.clone());
                found?
            }
        };
        let name = file
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut layer = Layer::new(LayerId(0), name, media, length, rate, channels);
        layer.key = region.key;
        layer.velocity = region.velocity;
        let root = match region.root {
            sfz::Root::Key(key) => i32::from(key),
            sfz::Root::File => i32::from(info.root_key.unwrap_or(0)),
        };
        layer.root_key = (root + region.root_shift).clamp(0, 127) as u8;
        // The reader keeps these inside the layers' ranges; `check` below
        // holds the layer to them.
        layer.detune = region.detune;
        layer.volume = region.volume.min(bl_model::LAYER_GAIN_MAX);
        layer.pan = region.pan;
        layer.start = region.offset.min(length - 1);
        layer.end = region.end.map_or(length, |end| end.min(length));
        if layer.start >= layer.end {
            return Err("its offset is at or past its end".to_owned());
        }
        // The loop is the region's, or else the file's first (R50).
        let file_loop = info
            .loop_points
            .filter(|(start, end)| start < end && *end <= length);
        let loop_end = region
            .loop_end
            .or(file_loop.map(|(_, end)| end))
            .unwrap_or(layer.end)
            .clamp(layer.start + 1, layer.end);
        let loop_start = region
            .loop_start
            .or(file_loop.map(|(start, _)| start))
            .unwrap_or(layer.start)
            .clamp(layer.start, loop_end - 1);
        layer.loop_start = loop_start;
        layer.loop_end = loop_end;
        layer.release_loop = loop_end;
        let play = region.playback(file_loop.is_some());
        layer.sustain = play.sustain;
        layer.release = play.release;
        layer.one_shot = play.one_shot;
        if let Some(seconds) = region.crossfade {
            layer.crossfade_ms = (seconds * 1000.0).min(layer.crossfade_limit_ms());
        }
        layer.check().map_err(|error| error.to_string())?;
        Ok(layer)
    }

    /// Imports an SFZ file as a multisampler's layers (R47 to R53).
    fn import_sfz(&mut self, id: &str, args: ImportSfzArgs) -> Result<Value, ShellError> {
        let path = Path::new(&args.path);
        let file = sfz::read(path)
            .map_err(|error| self.failed(id, format!("{}: {error}", path.display())))?;
        let existing = match args.device {
            Some(device) => Some(self.multisampler_target(id, Some(device))?),
            None => None,
        };
        if existing.is_none()
            && let Some(track) = args.track
            && self
                .editor
                .document()
                .track(track)
                .map(bl_model::Track::kind)
                != Some(TrackKind::Midi)
        {
            return Err(self.failed(id, format!("{track} isn't a MIDI track")));
        }
        let mut left_out: Vec<SfzLeftOut> = file
            .report
            .left_out
            .iter()
            .map(|out| SfzLeftOut {
                region: out.region,
                sample: out.sample.clone(),
                reason: out.reason.clone(),
            })
            .collect();
        let mut layers = Vec::new();
        let mut opened = HashMap::new();
        for region in &file.regions {
            match self.sfz_layer(id, region, &mut opened) {
                Ok(layer) => layers.push(layer),
                Err(reason) => left_out.push(SfzLeftOut {
                    region: region.number,
                    sample: region.sample_text.clone(),
                    reason,
                }),
            }
        }
        left_out.sort_by_key(|out| out.region);
        if layers.is_empty() {
            let why: Vec<String> = left_out
                .iter()
                .map(|out| format!("region {}: {}", out.region, out.reason))
                .collect();
            return Err(self.failed(
                id,
                if why.is_empty() {
                    format!("{} has no regions", path.display())
                } else {
                    format!("no region could be imported ({})", why.join("; "))
                },
            ));
        }
        let count = layers.len();
        let params = sfz_params(&file);
        let name = path
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (device, track) = match existing {
            Some(device) => {
                self.editor
                    .transact(ids::IMPORT_SFZ_STEP, |editor| {
                        editor.apply(Command::InsertLayers {
                            device,
                            index: None,
                            layers,
                        })
                    })
                    .map_err(|error| self.failed(id, error))?;
                let track = self
                    .editor
                    .document()
                    .device_track(device)
                    .ok_or_else(|| self.failed(id, format!("{device} isn't on a track")))?;
                (device, track)
            }
            None => self.insert_sfz_device(id, args.track, &name, params, layers)?,
        };
        let held = self.layers_of(device);
        let added: Vec<LayerId> = held.list[held.list.len() - count..]
            .iter()
            .map(|l| l.id)
            .collect();
        self.devices.select_layers(device, added.clone());
        self.devices.select_only(device);
        self.changed();
        let counts = |list: &[(String, usize)]| {
            list.iter()
                .map(|(name, regions)| SfzCount {
                    name: name.clone(),
                    regions: *regions,
                })
                .collect()
        };
        Ok(to_value(&SfzImported {
            device,
            track,
            layers: added,
            unread: counts(&file.report.unread),
            settled: counts(&file.report.majority),
            left_out,
        }))
    }

    /// Makes a multisampler named `name` with `params` and `layers` on
    /// `track`, or on a new MIDI track below the last, as one undo step.
    fn insert_sfz_device(
        &mut self,
        id: &str,
        track: Option<TrackId>,
        name: &str,
        params: Vec<(ParamId, f64)>,
        layers: Vec<Layer>,
    ) -> Result<(DeviceId, TrackId), ShellError> {
        let description = mosaic::multisampler_type().description;
        let result = self.editor.transact(ids::IMPORT_SFZ_STEP, |editor| {
            let track = match track {
                Some(track) => track,
                None => {
                    let track: TrackId = editor.new_id();
                    let top = editor.document().top_level().len();
                    editor.apply(Command::CreateTrack {
                        id: track,
                        kind: TrackKind::Midi,
                        name: name.to_owned(),
                        parent: None,
                        index: top,
                    })?;
                    track
                }
            };
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
                device: mosaic::multisampler_type().device(device),
            })?;
            for (param, value) in params {
                if let Some(descriptor) = description.param(param) {
                    editor.apply(Command::SetDeviceParam {
                        device,
                        param,
                        value: descriptor.clamp(value),
                    })?;
                }
            }
            editor.apply(Command::InsertLayers {
                device,
                index: None,
                layers,
            })?;
            Ok::<_, bl_model::EditError>((device, track))
        });
        let (device, track) = result.map_err(|error| self.failed(id, error))?;
        self.arrangement
            .run(
                bl_shell_spec::arrangement::SELECT_TRACKS,
                json!({ "tracks": [track] }),
                self.editor.document(),
            )
            .ok();
        Ok((device, track))
    }

    /// A layer's sample with its edits applied, at its own rate (R57).
    fn left_channel(&self, layer: &Layer) -> Option<std::sync::Arc<SampleData>> {
        self.edited_audio(layer.media, &layer.edits)
    }

    /// Changes `layers` one at a time inside one transaction: each gets the
    /// settings `settings_of` gives (the layer as it is, and `base`), its
    /// markers snapped to zero crossings if `snap` says.
    #[expect(clippy::too_many_arguments, reason = "one edit's parts")]
    fn edit_layers(
        &mut self,
        id: &str,
        label: &'static str,
        device: DeviceId,
        layers: &[LayerId],
        settings_of: impl Fn(&Layer, &LayerSettings) -> LayerSettings,
        base: &LayerSettings,
        snap: bool,
    ) -> Result<LayersChanged, ShellError> {
        let held = self.layers_of(device);
        let mut edits = Vec::new();
        for &layer_id in layers {
            let layer = held
                .get(layer_id)
                .ok_or_else(|| self.failed(id, format!("no layer {}", layer_id.0)))?;
            let mut settings = settings_of(layer, base);
            if snap && let Some(sample) = self.left_channel(layer) {
                let left = sample.channel(0).unwrap_or(&[]);
                let snapped = |marker: Option<u64>| {
                    marker.map(|m| snap_to_zero_crossing(left, m).min(layer.length))
                };
                settings.start = snapped(settings.start);
                settings.end = snapped(settings.end);
                settings.loop_start = snapped(settings.loop_start);
                settings.loop_end = snapped(settings.loop_end);
                settings.release_loop = snapped(settings.release_loop);
            }
            edits.push((layer_id, settings));
        }
        let result = self.editor.transact(label, |editor| {
            for (layer, settings) in edits {
                editor.apply(Command::UpdateLayers {
                    device,
                    layers: vec![layer],
                    settings,
                })?;
            }
            Ok::<_, bl_model::EditError>(())
        });
        result.map_err(|error| self.failed(id, error))?;
        self.changed();
        Ok(LayersChanged {
            device,
            layers: layers.to_vec(),
        })
    }

    /// Normalize Volume and Normalize Pan (R13).
    fn normalize(
        &mut self,
        id: &str,
        device: DeviceId,
        layers: &[LayerId],
        what: Normalize,
    ) -> Result<Value, ShellError> {
        let held = self.layers_of(device);
        let mut edits = Vec::new();
        for &layer_id in layers {
            let layer = held
                .get(layer_id)
                .ok_or_else(|| self.failed(id, format!("no layer {}", layer_id.0)))?;
            let Some(sample) = self.left_channel(layer) else {
                continue;
            };
            let (start, end) = (
                layer.start as usize,
                (layer.end as usize).min(sample.frames()),
            );
            let channel = |c: usize| {
                sample
                    .channel(c)
                    .map_or(&[][..], |s| &s[start.min(end)..end])
            };
            let settings = match what {
                Normalize::Volume => {
                    let peak = (0..sample.channel_count())
                        .flat_map(|c| channel(c).iter())
                        .fold(0.0f32, |peak, s| peak.max(s.abs()));
                    (peak > 0.0).then(|| LayerSettings {
                        volume: Some((1.0 / f64::from(peak)).min(bl_model::LAYER_GAIN_MAX)),
                        ..LayerSettings::default()
                    })
                }
                Normalize::Pan if sample.channel_count() >= 2 => {
                    let rms = |c: usize| {
                        let data = channel(c);
                        (data.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                            / data.len().max(1) as f64)
                            .sqrt()
                    };
                    let (left, right) = (rms(0), rms(1));
                    (left > 0.0 && right > 0.0).then(|| {
                        // Pan takes one side down by (1 - b) while the
                        // other stays: the louder channel comes down to
                        // the quieter one.
                        let balance = if left > right {
                            1.0 - right / left
                        } else {
                            left / right - 1.0
                        };
                        LayerSettings {
                            pan: Some(balance * 50.0),
                            ..LayerSettings::default()
                        }
                    })
                }
                Normalize::Pan => None,
            };
            if let Some(settings) = settings {
                edits.push((layer_id, settings));
            }
        }
        let label = match what {
            Normalize::Volume => "Normalize Volume",
            Normalize::Pan => "Normalize Pan",
        };
        self.editor
            .transact(label, |editor| {
                for (layer, settings) in edits {
                    editor.apply(Command::UpdateLayers {
                        device,
                        layers: vec![layer],
                        settings,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|error| self.failed(id, error))?;
        self.changed();
        Ok(to_value(&LayersChanged {
            device,
            layers: layers.to_vec(),
        }))
    }

    /// Changes the layer selection (R11).
    fn select(
        &mut self,
        id: &str,
        device: DeviceId,
        args: &SelectLayersArgs,
    ) -> Result<(), ShellError> {
        let held = self.layers_of(device);
        if let Some(missing) = args.layers.iter().find(|l| held.get(**l).is_none()) {
            return Err(self.failed(id, format!("no layer {}", missing.0)));
        }
        let now: BTreeSet<LayerId> = self
            .devices
            .layer_selection(self.editor.document(), device)
            .into_iter()
            .collect();
        let picked: BTreeSet<LayerId> = args.layers.iter().copied().collect();
        let next: BTreeSet<LayerId> = match args.mode {
            LayerSelectMode::Replace if args.layers.is_empty() => {
                held.list.iter().map(|l| l.id).collect()
            }
            LayerSelectMode::Replace => picked,
            LayerSelectMode::Add => &now | &picked,
            LayerSelectMode::Toggle => &now ^ &picked,
            LayerSelectMode::SameRange => {
                let kind = self.devices.multisampler_view(device).editor;
                let zone = |l: &Layer| match kind {
                    ZoneKind::Key => l.key,
                    ZoneKind::Velocity => l.velocity,
                    ZoneKind::Select => l.select,
                };
                let first = args
                    .layers
                    .first()
                    .and_then(|l| held.get(*l))
                    .ok_or_else(|| self.failed(id, "name a layer first"))?;
                let wanted = (zone(first).low, zone(first).high);
                held.list
                    .iter()
                    .filter(|l| (zone(l).low, zone(l).high) == wanted)
                    .map(|l| l.id)
                    .collect()
            }
        };
        let ordered: Vec<LayerId> = held
            .list
            .iter()
            .map(|l| l.id)
            .filter(|l| next.contains(l))
            .collect();
        self.devices.select_layers(device, ordered);
        Ok(())
    }

    /// A key of the computer keyboard went down or up: with Auto Select on,
    /// the multisamplers of the track the Device View shows select the layers
    /// the note plays while it's held (R7).
    pub(super) fn follow_note(&mut self, key: u8, velocity: u8, down: bool) {
        let Some(track) = devices::shown_track(&self.arrangement, self.editor.document()) else {
            return;
        };
        self.devices
            .note(self.editor.document(), track, key, velocity, down);
        self.touch();
    }

    /// The folder a layer's sample lies in, for Show in Browser (R11).
    #[must_use]
    pub fn layer_folder(&self, media: ContentHash) -> Option<PathBuf> {
        self.media
            .entry(bl_media::ContentHash::from_bytes(media.0))
            .and_then(|entry| entry.path.parent().map(Path::to_path_buf))
    }

    /// The device a command found, for tests and the UI.
    #[must_use]
    pub fn multisampler_device(&self, device: DeviceId) -> Option<&Device> {
        self.editor
            .document()
            .device(device)
            .filter(|d| DeviceView::is_multisampler(d))
    }
}

/// A sample file an SFZ import has opened: its media, length, rate and
/// channels, and what its sampler chunk says.
type OpenedSample = (ContentHash, u64, u32, u8, bl_media::SamplerInfo);

/// The multisampler's parameters an SFZ file's device-wide settings give
/// (R51, R52).
fn sfz_params(file: &sfz::Sfz) -> Vec<(ParamId, f64)> {
    let d = &file.device;
    let mut params = Vec::new();
    let mut set = |param: ParamId, value: Option<f64>| {
        if let Some(value) = value {
            params.push((param, value));
        }
    };
    set(mosaic::VEL_VOL, d.vel_vol);
    set(mosaic::ENV_ATTACK, d.volume_env.attack);
    set(mosaic::ENV_DECAY, d.volume_env.decay);
    set(mosaic::ENV_SUSTAIN, d.volume_env.sustain);
    set(mosaic::ENV_RELEASE, d.volume_env.release);
    if let Some(filter) = d.filter {
        set(mosaic::FILTER_ON, Some(1.0));
        set(
            mosaic::FILTER_TYPE,
            Some(match filter.kind {
                FilterKind::LowPass => 0.0,
                FilterKind::HighPass => 1.0,
                FilterKind::BandPass => 2.0,
                FilterKind::Notch => 3.0,
            }),
        );
        set(
            mosaic::FILTER_SLOPE,
            Some(f64::from(u8::from(filter.steep))),
        );
        set(mosaic::FILTER_FREQ, Some(filter.cutoff));
        set(mosaic::FILTER_RES, Some(filter.resonance));
    }
    // An envelope that goes down is the same envelope turned upside down.
    for (env, ids, amount, on) in [
        (
            &d.filter_env,
            mosaic::FILTER_ENV,
            mosaic::FENV_AMOUNT,
            Some(mosaic::FENV_ON),
        ),
        (
            &d.pitch_env,
            mosaic::PITCH_ENV,
            mosaic::PITCH_ENV_AMOUNT,
            None,
        ),
    ] {
        let Some(depth) = env.depth.filter(|depth| *depth != 0.0) else {
            continue;
        };
        let sign = depth.signum();
        set(amount, Some(if on.is_some() { depth.abs() } else { depth }));
        if let Some(on) = on {
            set(on, Some(1.0));
        }
        set(
            ids.peak,
            Some(100.0 * if on.is_some() { sign } else { 1.0 }),
        );
        set(
            ids.sustain,
            env.sustain
                .map(|s| s * if on.is_some() { sign } else { 1.0 }),
        );
        set(ids.attack, env.attack);
        set(ids.decay, env.decay);
        set(ids.release, env.release);
    }
    set(mosaic::BEND_RANGE, d.bend_range);
    set(mosaic::REVERSE, d.reverse.then_some(1.0));
    set(
        mosaic::FADE_MODE,
        d.fade_power.map(|power| f64::from(u8::from(power))),
    );
    if let Some(mode) = file.round_robin {
        set(mosaic::ROUND_ROBIN, Some(1.0));
        set(
            mosaic::RR_MODE,
            Some(match mode {
                RoundRobin::Forward => 0.0,
                RoundRobin::Random => 3.0,
            }),
        );
    }
    params
}
